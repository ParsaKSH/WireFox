use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use tracing::info;

use super::{error, ApiResult, ApiState};
use crate::settings::{Settings, SettingsPatch};

pub async fn get(State(st): State<Arc<ApiState>>) -> ApiResult<Settings> {
    Ok(Json(st.settings.get()))
}

/// Takes the raw body so a bad strategy name or a negative timeout is a 400
/// with a JSON error, like a value out of range, rather than axum's plain-text 422.
pub async fn update(State(st): State<Arc<ApiState>>, body: Bytes) -> ApiResult<Settings> {
    let patch: SettingsPatch =
        serde_json::from_slice(&body).map_err(|e| error(StatusCode::BAD_REQUEST, format!("invalid settings: {e}")))?;
    let settings = st.settings.apply(patch).map_err(|e| error(StatusCode::BAD_REQUEST, e))?;
    info!(balance = ?settings.balance, settings.handshake_timeout_secs, settings.connect_timeout_secs, "settings changed");
    Ok(Json(settings))
}
