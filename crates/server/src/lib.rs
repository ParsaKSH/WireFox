//! WireFox server: the SOCKS5 front end, the hub nodes tunnel into, and the
//! control API.

pub mod api;
pub mod balance;
pub mod config;
pub mod limiter;
pub mod socks;
pub mod state;
pub mod tunnel;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio_rustls::TlsAcceptor;
use tracing::{info, warn};
use wirefox_common::net::bind_listener;

use crate::config::ServerConfig;
use crate::limiter::SocksAuthLimiter;
use crate::state::Store;

/// A running server. The listeners live on in background tasks.
pub struct Server {
    pub store: Arc<Store>,
    pub fingerprint: String,
    pub tunnel_addr: SocketAddr,
    pub socks_addrs: Vec<SocketAddr>,
    pub socks_tls_addrs: Vec<SocketAddr>,
    pub api_addr: SocketAddr,
}

/// Bind every listener and start serving. Port 0 picks a free port; the actual
/// addresses are in the returned [`Server`].
pub async fn start(cfg: ServerConfig) -> Result<Server> {
    let cert = wirefox_common::tls::load_or_generate(&cfg.cert_path(), &cfg.key_path(), &cfg.tls.sans)?;
    let fingerprint = cert.fingerprint();
    let tunnel_tls = TlsAcceptor::from(wirefox_common::tls::server_config(&cert, true)?);
    let public_tls = TlsAcceptor::from(wirefox_common::tls::server_config(&cert, false)?);
    let store = Store::load(cfg.state_path())?;

    let bind = |addr: SocketAddr, what: &str| {
        let listener = bind_listener(addr).with_context(|| format!("binding {what} listener on {addr}"))?;
        let local = listener.local_addr()?;
        info!(%local, "{what} listener up");
        anyhow::Ok((listener, local))
    };

    let (listener, tunnel_addr) = bind(cfg.tunnel.listen, "tunnel")?;
    tokio::spawn(tunnel::serve(listener, tunnel_tls, store.clone()));

    let socks_ctx = socks::SocksCtx {
        store: store.clone(),
        limiter: Arc::new(SocksAuthLimiter::new(Duration::from_secs(300))),
        balance: cfg.socks.balance,
        handshake_timeout: Duration::from_secs(cfg.socks.handshake_timeout_secs),
        connect_timeout: Duration::from_secs(cfg.socks.connect_timeout_secs),
    };
    let mut socks_addrs = Vec::new();
    for &addr in &cfg.socks.listen {
        let (listener, local) = bind(addr, "socks5")?;
        tokio::spawn(socks::serve(listener, None, socks_ctx.clone()));
        socks_addrs.push(local);
    }
    let mut socks_tls_addrs = Vec::new();
    for &addr in &cfg.socks.tls_listen {
        let (listener, local) = bind(addr, "socks5+tls")?;
        tokio::spawn(socks::serve(listener, Some(public_tls.clone()), socks_ctx.clone()));
        socks_tls_addrs.push(local);
    }

    let public_tunnel_addr = match &cfg.tunnel.public_addr {
        Some(addr) => addr.clone(),
        None if tunnel_addr.ip().is_unspecified() => {
            warn!("tunnel.public_addr is not set; generated node configs will need the server address filled in");
            format!("<server-address>:{}", tunnel_addr.port())
        }
        None => tunnel_addr.to_string(),
    };
    let api_state =
        api::ApiState::new(store.clone(), cfg.api.clone(), fingerprint.clone(), public_tunnel_addr, cfg.socks.balance)?;
    let app = api::router(api_state)?;
    let (listener, api_addr) = bind(cfg.api.listen, "api")?;
    tokio::spawn(api::serve(listener, cfg.api.tls.then(|| public_tls.clone()), app));

    tokio::spawn(store.clone().autosave());
    Ok(Server { store, fingerprint, tunnel_addr, socks_addrs, socks_tls_addrs, api_addr })
}
