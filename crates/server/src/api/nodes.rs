use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;
use wirefox_common::proto::{EgressSpec, ServerMsg};

use super::{default_true, double_option, error, ok, valid_id, ApiError, ApiResult, ApiState};
use crate::state::{now_secs, NodeCfg, NodeEntry, TrafficSnapshot};
use crate::tunnel::SessionSnapshot;

#[derive(Serialize)]
pub struct NodeView {
    id: String,
    name: String,
    enabled: bool,
    egress_override: Option<EgressSpec>,
    created_at: u64,
    online: bool,
    traffic: TrafficSnapshot,
    session: Option<SessionSnapshot>,
}

impl NodeView {
    fn of(node: &NodeEntry) -> Self {
        let cfg = node.cfg.read().clone();
        let session = node.online_session().map(|s| s.snapshot());
        NodeView {
            id: node.id.clone(),
            name: cfg.name,
            enabled: cfg.enabled,
            egress_override: cfg.egress,
            created_at: cfg.created_at,
            online: session.is_some(),
            traffic: node.stats.snapshot(),
            session,
        }
    }
}

fn find(st: &ApiState, id: &str) -> Result<Arc<NodeEntry>, ApiError> {
    st.store.node(id).ok_or_else(|| error(StatusCode::NOT_FOUND, format!("node {id:?} not found")))
}

fn config_snippet(st: &ApiState, id: &str, token: &str) -> String {
    format!(
        "server = \"{}\"\nserver_fingerprint = \"{}\"\nnode_id = \"{id}\"\ntoken = \"{token}\"\n",
        st.public_tunnel_addr, st.fingerprint
    )
}

/// Drop the node's session and every stream going through it.
fn cut_off(st: &ApiState, node: &NodeEntry) {
    node.disconnect();
    st.store.kill_conns(|c| c.node == node.id);
}

pub async fn list(State(st): State<Arc<ApiState>>) -> ApiResult<Vec<NodeView>> {
    let mut nodes: Vec<_> = st.store.nodes.read().values().map(|n| NodeView::of(n)).collect();
    nodes.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(nodes))
}

pub async fn get(State(st): State<Arc<ApiState>>, Path(id): Path<String>) -> ApiResult<NodeView> {
    let node = find(&st, &id)?;
    Ok(Json(NodeView::of(&node)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateNode {
    id: Option<String>,
    name: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    egress: Option<EgressSpec>,
}

pub async fn create(State(st): State<Arc<ApiState>>, Json(body): Json<CreateNode>) -> ApiResult<serde_json::Value> {
    let id = body.id.unwrap_or_else(|| format!("node-{}", wirefox_common::random_token(4)));
    if !valid_id(&id) {
        return Err(error(StatusCode::BAD_REQUEST, "id must be 1-64 characters of [A-Za-z0-9._-]"));
    }
    let token = wirefox_common::random_token(32);
    let cfg = NodeCfg {
        name: body.name.unwrap_or_else(|| id.clone()),
        token_hash: wirefox_common::sha256_hex(token.as_bytes()),
        enabled: body.enabled,
        egress: body.egress.filter(|e| !e.is_default()),
        created_at: now_secs(),
    };
    if !st.store.insert_node(id.clone(), cfg) {
        return Err(error(StatusCode::CONFLICT, format!("node {id:?} already exists")));
    }
    st.store.mark_dirty();
    info!(node = %id, "node created");
    let node = find(&st, &id)?;
    Ok(Json(json!({
        "node": NodeView::of(&node),
        "token": token,
        "node_config": config_snippet(&st, &id, &token),
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateNode {
    name: Option<String>,
    enabled: Option<bool>,
    /// `null` clears the override.
    #[serde(default, deserialize_with = "double_option")]
    egress: Option<Option<EgressSpec>>,
}

pub async fn update(
    State(st): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateNode>,
) -> ApiResult<NodeView> {
    let node = find(&st, &id)?;
    let disabled = {
        let mut cfg = node.cfg.write();
        if let Some(name) = body.name {
            cfg.name = name;
        }
        if let Some(egress) = body.egress {
            cfg.egress = egress.filter(|e| !e.is_default());
            // Sent while holding the config lock, so it can't race with the
            // egress a reconnecting node is given.
            if let Some(session) = node.online_session() {
                session.send(ServerMsg::SetEgress { egress: cfg.egress.clone() });
            }
        }
        let was_enabled = cfg.enabled;
        if let Some(enabled) = body.enabled {
            cfg.enabled = enabled;
        }
        was_enabled && !cfg.enabled
    };
    if disabled {
        cut_off(&st, &node);
    }
    st.store.mark_dirty();
    Ok(Json(NodeView::of(&node)))
}

pub async fn remove(State(st): State<Arc<ApiState>>, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let node = st
        .store
        .nodes
        .write()
        .remove(&id)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("node {id:?} not found")))?;
    cut_off(&st, &node);
    st.store.mark_dirty();
    info!(node = %id, "node deleted");
    ok()
}

pub async fn rotate_token(State(st): State<Arc<ApiState>>, Path(id): Path<String>) -> ApiResult<serde_json::Value> {
    let node = find(&st, &id)?;
    let token = wirefox_common::random_token(32);
    node.cfg.write().token_hash = wirefox_common::sha256_hex(token.as_bytes());
    cut_off(&st, &node);
    st.store.mark_dirty();
    info!(node = %id, "node token rotated");
    Ok(Json(json!({ "token": token, "node_config": config_snippet(&st, &id, &token) })))
}

pub async fn refresh_interfaces(
    State(st): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let node = find(&st, &id)?;
    let session = node.online_session().ok_or_else(|| error(StatusCode::CONFLICT, "node is offline"))?;
    session.send(ServerMsg::RefreshInterfaces);
    ok()
}

pub async fn reset_traffic(State(st): State<Arc<ApiState>>, Path(id): Path<String>) -> ApiResult<NodeView> {
    let node = find(&st, &id)?;
    node.stats.reset();
    st.store.mark_dirty();
    Ok(Json(NodeView::of(&node)))
}
