use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use super::{bearer, error, ok, ApiResult, ApiState};
use crate::limiter::client_key;
use crate::state::now_secs;

#[derive(Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

pub async fn login(
    State(st): State<Arc<ApiState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<serde_json::Value> {
    let Some((admin_username, admin_password_hash)) = st.cfg.admin() else {
        return Err(error(StatusCode::NOT_FOUND, "admin login is disabled"));
    };
    let ip = peer.ip();
    let client = client_key(peer.ip());
    if st.login_failures.is_blocked(&client) {
        return Err(error(StatusCode::TOO_MANY_REQUESTS, "too many failed logins, try again later"));
    }
    let Ok(slot) = st.login_slots.clone().try_acquire_owned() else {
        return Err(error(StatusCode::TOO_MANY_REQUESTS, "too many logins in progress, try again"));
    };
    // Count the attempt before checking it, so parallel guesses can't get past the limit.
    st.login_failures.record_failure(client);

    // Always run the slow hash, so a wrong username takes as long as a wrong password.
    let hash = admin_password_hash.to_string();
    let password = body.password;
    let password_ok = tokio::task::spawn_blocking(move || {
        // Held until the hash is done, even if the client hangs up meanwhile.
        let _slot = slot;
        PasswordHash::new(&hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
    })
    .await
    .unwrap_or(false);
    let username_ok = wirefox_common::ct_eq(body.username.as_bytes(), admin_username.as_bytes());
    if !(password_ok && username_ok) {
        warn!(%ip, "failed API login");
        return Err(error(StatusCode::UNAUTHORIZED, "invalid credentials"));
    }
    st.login_failures.record_success(&client);

    let token = wirefox_common::random_token(32);
    let ttl = Duration::from_secs(st.cfg.session_ttl_secs);
    {
        let now = Instant::now();
        let mut sessions = st.sessions.lock();
        sessions.retain(|_, expiry| *expiry > now);
        sessions.insert(wirefox_common::sha256(token.as_bytes()), now + ttl);
    }
    info!(%ip, "API login");
    Ok(Json(json!({ "token": token, "expires_at": now_secs() + ttl.as_secs() })))
}

pub async fn logout(State(st): State<Arc<ApiState>>, req: Request) -> ApiResult<serde_json::Value> {
    if let Some(token) = bearer(req.headers()) {
        st.sessions.lock().remove(&wirefox_common::sha256(token.as_bytes()));
    }
    ok()
}
