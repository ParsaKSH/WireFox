//! REST API under `/api/v1`. Everything except `/auth/login` and `/health`
//! needs `Authorization: Bearer <token>`, where the token is a session token
//! from `/auth/login` or one of the configured API keys.

mod auth;
mod conns;
mod nodes;
mod users;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use argon2::PasswordHash;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use hyper_util::rt::{TokioIo, TokioTimer};
use parking_lot::Mutex;
use serde::{Deserialize, Deserializer};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tracing::debug;
use wirefox_common::net::accept;

use crate::config::ApiSection;
use crate::limiter::{client_key, FailureLimiter};
use crate::state::Store;

const MAX_CONNECTIONS: usize = 256;
const MAX_CONNECTIONS_PER_CLIENT: usize = 16;
/// Covers reading the body too, so a client can't trickle a request forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Argon2 needs ~19 MiB per run, so only a couple may run at once.
const CONCURRENT_LOGINS: usize = 2;

pub struct ApiState {
    pub store: Arc<Store>,
    pub cfg: ApiSection,
    pub fingerprint: String,
    /// Tunnel address (`host:port`) to put in generated node configs.
    pub public_tunnel_addr: String,
    pub started: Instant,
    /// SHA-256 of session token -> expiry.
    sessions: Mutex<HashMap<[u8; 32], Instant>>,
    api_keys: Vec<[u8; 32]>,
    login_failures: FailureLimiter<IpAddr>,
    login_slots: Arc<Semaphore>,
}

impl ApiState {
    pub fn new(
        store: Arc<Store>,
        cfg: ApiSection,
        fingerprint: String,
        public_tunnel_addr: String,
    ) -> anyhow::Result<Arc<Self>> {
        let api_keys = cfg
            .api_key_hashes
            .iter()
            .map(|h| {
                let bytes = hex::decode(h.trim())?;
                bytes.try_into().map_err(|_| anyhow::anyhow!("API key hashes must be 64 hex characters"))
            })
            .collect::<anyhow::Result<Vec<[u8; 32]>>>()?;
        PasswordHash::new(&cfg.admin_password_hash)
            .map_err(|e| anyhow::anyhow!("api.admin_password_hash is not a valid Argon2 hash: {e}"))?;
        Ok(Arc::new(ApiState {
            store,
            cfg,
            fingerprint,
            public_tunnel_addr,
            started: Instant::now(),
            sessions: Mutex::new(HashMap::new()),
            api_keys,
            login_failures: FailureLimiter::new(10, Duration::from_secs(300)),
            login_slots: Arc::new(Semaphore::new(CONCURRENT_LOGINS)),
        }))
    }

    fn token_valid(&self, token: &str) -> bool {
        let hash = wirefox_common::sha256(token.as_bytes());
        if self.sessions.lock().get(&hash).is_some_and(|expiry| *expiry > Instant::now()) {
            return true;
        }
        self.api_keys.iter().any(|key| wirefox_common::ct_eq(key, &hash))
    }
}

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn error(code: StatusCode, msg: impl Into<String>) -> ApiError {
    ApiError(code, msg.into())
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn ok() -> ApiResult<serde_json::Value> {
    Ok(Json(json!({ "ok": true })))
}

/// For PATCH bodies: tells "field absent" (`None`) apart from "field is null" (`Some(None)`).
fn double_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

fn default_true() -> bool {
    true
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::AUTHORIZATION)?.to_str().ok()?.strip_prefix("Bearer ").map(str::trim)
}

pub fn router(st: Arc<ApiState>) -> anyhow::Result<Router> {
    let protected = Router::new()
        .route("/status", get(status))
        .route("/auth/logout", post(auth::logout))
        .route("/nodes", get(nodes::list).post(nodes::create))
        .route("/nodes/{id}", get(nodes::get).patch(nodes::update).delete(nodes::remove))
        .route("/nodes/{id}/rotate-token", post(nodes::rotate_token))
        .route("/nodes/{id}/refresh-interfaces", post(nodes::refresh_interfaces))
        .route("/nodes/{id}/reset-traffic", post(nodes::reset_traffic))
        .route("/users", get(users::list).post(users::create))
        .route("/users/{name}", get(users::get).patch(users::update).delete(users::remove))
        .route("/users/{name}/reset-traffic", post(users::reset_traffic))
        .route("/connections", get(conns::list))
        .route("/connections/{id}", delete(conns::kill))
        .route_layer(middleware::from_fn_with_state(st.clone(), require_auth));
    let public = Router::new()
        .route("/auth/login", post(auth::login))
        .route("/health", get(|| async { Json(json!({ "ok": true })) }));

    let mut app = Router::new()
        .nest("/api/v1", protected.merge(public))
        .with_state(st.clone())
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, REQUEST_TIMEOUT));
    if !st.cfg.cors_origins.is_empty() {
        let origins = st.cfg.cors_origins.iter().map(|o| HeaderValue::from_str(o)).collect::<Result<Vec<_>, _>>()?;
        app = app.layer(
            tower_http::cors::CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
                .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]),
        );
    }
    Ok(app)
}

async fn status(State(st): State<Arc<ApiState>>) -> ApiResult<serde_json::Value> {
    let store = &st.store;
    let (nodes_total, nodes_online, bytes_up, bytes_down) = {
        let nodes = store.nodes.read();
        let online = nodes.values().filter(|n| n.online_session().is_some()).count();
        let up: u64 = nodes.values().map(|n| n.stats.up.load(Ordering::Relaxed)).sum();
        let down: u64 = nodes.values().map(|n| n.stats.down.load(Ordering::Relaxed)).sum();
        (nodes.len(), online, up, down)
    };
    Ok(Json(json!({
        "version": wirefox_common::VERSION,
        "uptime_secs": st.started.elapsed().as_secs(),
        "tunnel_fingerprint": st.fingerprint,
        "nodes_total": nodes_total,
        "nodes_online": nodes_online,
        "users_total": store.users.read().len(),
        "active_connections": store.conns.lock().len(),
        "bytes_up": bytes_up,
        "bytes_down": bytes_down,
    })))
}

async fn require_auth(State(st): State<Arc<ApiState>>, req: Request, next: Next) -> Response {
    match bearer(req.headers()) {
        Some(token) if st.token_valid(token) => next.run(req).await,
        _ => error(StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response(),
    }
}

/// Limits on concurrent API connections, overall and per client.
struct ConnLimits {
    total: Arc<Semaphore>,
    per_client: Mutex<HashMap<IpAddr, usize>>,
}

/// One admitted connection; gives its slots back when dropped.
struct ConnSlot {
    limits: Arc<ConnLimits>,
    client: IpAddr,
    _total: OwnedSemaphorePermit,
}

impl ConnLimits {
    fn admit(self: &Arc<Self>, ip: IpAddr) -> Option<ConnSlot> {
        let total = self.total.clone().try_acquire_owned().ok()?;
        let client = client_key(ip);
        let mut per_client = self.per_client.lock();
        let count = per_client.entry(client).or_default();
        if *count >= MAX_CONNECTIONS_PER_CLIENT {
            return None;
        }
        *count += 1;
        Some(ConnSlot { limits: self.clone(), client, _total: total })
    }
}

impl Drop for ConnSlot {
    fn drop(&mut self) {
        let mut per_client = self.limits.per_client.lock();
        if let Some(count) = per_client.get_mut(&self.client) {
            *count -= 1;
            if *count == 0 {
                per_client.remove(&self.client);
            }
        }
    }
}

/// Serve the API over HTTP/1.1, optionally with TLS.
pub async fn serve(listener: TcpListener, tls: Option<TlsAcceptor>, app: Router) {
    let limits =
        Arc::new(ConnLimits { total: Arc::new(Semaphore::new(MAX_CONNECTIONS)), per_client: Default::default() });
    loop {
        let (stream, peer) = accept(&listener).await;
        let Some(slot) = limits.admit(peer.ip()) else {
            debug!(%peer, "too many API connections, dropping");
            continue;
        };
        let app = app.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _slot = slot;
            let service = hyper::service::service_fn(move |mut req: hyper::Request<hyper::body::Incoming>| {
                req.extensions_mut().insert(ConnectInfo(peer));
                app.clone().oneshot(req)
            });
            // HTTP/1 only: its header timeout covers idle connections from the
            // moment they are accepted, which HTTP/2 has no equivalent for.
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder.timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
            let result = match tls {
                None => builder.serve_connection(TokioIo::new(stream), service).await,
                Some(acceptor) => match tokio::time::timeout(TLS_HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                    Ok(Ok(stream)) => builder.serve_connection(TokioIo::new(stream), service).await,
                    _ => return,
                },
            };
            if let Err(e) = result {
                debug!(%peer, error = %e, "api connection error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Patch {
        #[serde(default, deserialize_with = "double_option")]
        note: Option<Option<String>>,
    }

    #[test]
    fn double_option_semantics() {
        let absent: Patch = serde_json::from_str("{}").unwrap();
        let null: Patch = serde_json::from_str(r#"{"note":null}"#).unwrap();
        let value: Patch = serde_json::from_str(r#"{"note":"hi"}"#).unwrap();
        assert_eq!(absent.note, None);
        assert_eq!(null.note, Some(None));
        assert_eq!(value.note, Some(Some("hi".into())));
    }

    #[test]
    fn ids() {
        assert!(valid_id("node-1"));
        assert!(valid_id("a.b_c"));
        assert!(!valid_id(""));
        assert!(!valid_id("has space"));
        assert!(!valid_id("../etc"));
        assert!(!valid_id(&"x".repeat(65)));
    }

    #[test]
    fn bearer_parsing() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer abc "));
        assert_eq!(bearer(&headers), Some("abc"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer(&headers), None);
    }
}
