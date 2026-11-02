use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use super::{error, ok, ApiResult, ApiState};
use crate::state::ConnSnapshot;

#[derive(Deserialize)]
pub struct Filter {
    user: Option<String>,
    node: Option<String>,
}

pub async fn list(State(st): State<Arc<ApiState>>, Query(filter): Query<Filter>) -> ApiResult<Vec<ConnSnapshot>> {
    let mut conns: Vec<_> = st
        .store
        .conns
        .lock()
        .values()
        .filter(|c| filter.user.as_ref().is_none_or(|u| &c.user == u))
        .filter(|c| filter.node.as_ref().is_none_or(|n| &c.node == n))
        .map(|c| c.snapshot())
        .collect();
    conns.sort_by_key(|c| c.id);
    Ok(Json(conns))
}

pub async fn kill(State(st): State<Arc<ApiState>>, Path(id): Path<u64>) -> ApiResult<serde_json::Value> {
    match st.store.kill_conns(|c| c.id == id) {
        0 => Err(error(StatusCode::NOT_FOUND, format!("connection {id} not found"))),
        _ => ok(),
    }
}
