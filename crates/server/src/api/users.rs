use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::info;

use super::{default_true, double_option, error, ok, valid_id, ApiError, ApiResult, ApiState};
use crate::state::{now_secs, SocksPassword, TrafficSnapshot, UserCfg, UserEntry};

#[derive(Serialize)]
pub struct UserView {
    username: String,
    enabled: bool,
    nodes: Vec<String>,
    max_connections: Option<u32>,
    note: Option<String>,
    created_at: u64,
    traffic: TrafficSnapshot,
}

impl UserView {
    fn of(user: &UserEntry) -> Self {
        let cfg = user.cfg.read().clone();
        UserView {
            username: user.username.clone(),
            enabled: cfg.enabled,
            nodes: cfg.nodes,
            max_connections: cfg.max_connections,
            note: cfg.note,
            created_at: cfg.created_at,
            traffic: user.stats.snapshot(),
        }
    }
}

fn find(st: &ApiState, name: &str) -> Result<Arc<UserEntry>, ApiError> {
    st.store.user(name).ok_or_else(|| error(StatusCode::NOT_FOUND, format!("user {name:?} not found")))
}

fn check_nodes_exist(st: &ApiState, nodes: &[String]) -> Result<(), ApiError> {
    let known = st.store.nodes.read();
    match nodes.iter().find(|n| !known.contains_key(*n)) {
        Some(n) => Err(error(StatusCode::BAD_REQUEST, format!("unknown node {n:?}"))),
        None => Ok(()),
    }
}

fn check_password(p: &str) -> Result<(), ApiError> {
    // SOCKS5 sends the password with a one-byte length.
    if p.is_empty() || p.len() > 255 {
        return Err(error(StatusCode::BAD_REQUEST, "password must be 1-255 bytes"));
    }
    Ok(())
}

pub async fn list(State(st): State<Arc<ApiState>>) -> ApiResult<Vec<UserView>> {
    let mut users: Vec<_> = st.store.users.read().values().map(|u| UserView::of(u)).collect();
    users.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(Json(users))
}

pub async fn get(State(st): State<Arc<ApiState>>, Path(name): Path<String>) -> ApiResult<UserView> {
    let user = find(&st, &name)?;
    Ok(Json(UserView::of(&user)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateUser {
    username: String,
    /// Generated (and returned once) when omitted.
    password: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    nodes: Vec<String>,
    max_connections: Option<u32>,
    note: Option<String>,
}

pub async fn create(State(st): State<Arc<ApiState>>, Json(body): Json<CreateUser>) -> ApiResult<serde_json::Value> {
    if !valid_id(&body.username) {
        return Err(error(StatusCode::BAD_REQUEST, "username must be 1-64 characters of [A-Za-z0-9._-]"));
    }
    check_nodes_exist(&st, &body.nodes)?;
    let generated = body.password.is_none();
    let password = body.password.unwrap_or_else(|| wirefox_common::random_token(12));
    check_password(&password)?;
    let cfg = UserCfg {
        password: SocksPassword::new(&password),
        enabled: body.enabled,
        nodes: body.nodes,
        max_connections: body.max_connections,
        note: body.note,
        created_at: now_secs(),
    };
    if !st.store.insert_user(body.username.clone(), cfg) {
        return Err(error(StatusCode::CONFLICT, format!("user {:?} already exists", body.username)));
    }
    st.store.mark_dirty();
    info!(user = %body.username, "user created");
    let user = find(&st, &body.username)?;
    let mut response = json!({ "user": UserView::of(&user) });
    if generated {
        response["password"] = json!(password);
    }
    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateUser {
    password: Option<String>,
    enabled: Option<bool>,
    nodes: Option<Vec<String>>,
    #[serde(default, deserialize_with = "double_option")]
    max_connections: Option<Option<u32>>,
    #[serde(default, deserialize_with = "double_option")]
    note: Option<Option<String>>,
}

pub async fn update(
    State(st): State<Arc<ApiState>>,
    Path(name): Path<String>,
    Json(body): Json<UpdateUser>,
) -> ApiResult<UserView> {
    let user = find(&st, &name)?;
    if let Some(nodes) = &body.nodes {
        check_nodes_exist(&st, nodes)?;
    }
    if let Some(p) = &body.password {
        check_password(p)?;
    }
    let password_changed = body.password.is_some();
    let (enabled, allowed_nodes) = {
        let mut cfg = user.cfg.write();
        if let Some(p) = body.password {
            cfg.password = SocksPassword::new(&p);
        }
        if let Some(enabled) = body.enabled {
            cfg.enabled = enabled;
        }
        if let Some(nodes) = body.nodes {
            cfg.nodes = nodes;
        }
        if let Some(max) = body.max_connections {
            cfg.max_connections = max;
        }
        if let Some(note) = body.note {
            cfg.note = note;
        }
        (cfg.enabled, cfg.nodes.clone())
    };
    // Cut streams the user is no longer allowed to have. A new password also
    // ends the streams opened with the old one.
    st.store.kill_conns(|c| {
        c.user == name
            && (password_changed || !enabled || (!allowed_nodes.is_empty() && !allowed_nodes.contains(&c.node)))
    });
    st.store.mark_dirty();
    Ok(Json(UserView::of(&user)))
}

pub async fn remove(State(st): State<Arc<ApiState>>, Path(name): Path<String>) -> ApiResult<serde_json::Value> {
    st.store
        .users
        .write()
        .remove(&name)
        .ok_or_else(|| error(StatusCode::NOT_FOUND, format!("user {name:?} not found")))?;
    st.store.kill_conns(|c| c.user == name);
    st.store.mark_dirty();
    info!(user = %name, "user deleted");
    ok()
}

pub async fn reset_traffic(State(st): State<Arc<ApiState>>, Path(name): Path<String>) -> ApiResult<UserView> {
    let user = find(&st, &name)?;
    user.stats.reset();
    st.store.mark_dirty();
    Ok(Json(UserView::of(&user)))
}
