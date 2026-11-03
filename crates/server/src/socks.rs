//! SOCKS5 front end: RFC 1928 CONNECT with RFC 1929 username/password auth,
//! plain or wrapped in TLS.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};
use wirefox_common::net::{accept, relay, tune_tcp};
use wirefox_common::proto::{self, read_short_string, rep, Target};

use crate::balance::Candidate;
use crate::limiter::SocksAuthLimiter;
use crate::settings::LiveSettings;
use crate::state::{ConnEntry, Store, Traffic, UserEntry};
use crate::tunnel::{NodeSession, TunnelConn};

const SOCKS_VERSION: u8 = 5;
const METHOD_USERPASS: u8 = 2;
const METHOD_NONE_ACCEPTABLE: u8 = 0xFF;
const USERPASS_VERSION: u8 = 1;
const AUTH_OK: u8 = 0;
const AUTH_FAILED: u8 = 1;
const CMD_CONNECT: u8 = 1;
/// A pooled tunnel connection can turn out to be dead; try a few before giving up.
const STALE_RETRIES: usize = 3;

#[derive(Clone)]
pub struct SocksCtx {
    pub store: Arc<Store>,
    pub limiter: Arc<SocksAuthLimiter>,
    /// Read once per connection, so API changes apply to new connections.
    pub settings: Arc<LiveSettings>,
}

pub async fn serve(listener: TcpListener, tls: Option<TlsAcceptor>, ctx: SocksCtx) {
    loop {
        let (stream, peer) = accept(&listener).await;
        let ctx = ctx.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            let _ = tune_tcp(&stream);
            let result = match tls {
                None => handle(stream, peer, ctx).await,
                Some(acceptor) => {
                    let timeout = ctx.settings.get().handshake_timeout();
                    match tokio::time::timeout(timeout, acceptor.accept(stream)).await {
                        Ok(Ok(stream)) => handle(stream, peer, ctx).await,
                        Ok(Err(e)) => Err(e),
                        Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out")),
                    }
                }
            };
            if let Err(e) = result {
                debug!(%peer, error = %e, "socks connection closed");
            }
        });
    }
}

/// Why a request could not be connected; maps onto a SOCKS reply code.
enum ConnectError {
    /// The node answered with this reply code.
    Refused(u8),
    /// The node's session went away; another node may still work.
    SessionGone,
    TimedOut,
    Io(io::Error),
}

async fn handle<S>(mut client: S, peer: SocketAddr, ctx: SocksCtx) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let settings = ctx.settings.get();
    let (user, target) = tokio::time::timeout(settings.handshake_timeout(), handshake(&mut client, peer, &ctx))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "handshake timed out"))??;

    let max_connections = user.cfg.read().max_connections;
    let Some(_user_slot) = user.stats.try_enter(max_connections) else {
        reply_error(&mut client, rep::NOT_ALLOWED).await;
        return Err(io::Error::other(format!("user {} reached max_connections", user.username)));
    };

    let deadline = Instant::now() + settings.connect_timeout();
    let mut tried = Vec::new();
    loop {
        let Some(Candidate { node, session }) = ctx.store.pick_node(&user, settings.balance, &tried) else {
            reply_error(&mut client, rep::NETWORK_UNREACHABLE).await;
            return Err(io::Error::other(format!("no node online for user {}", user.username)));
        };
        tried.push(node.id.clone());
        // Register first and check second: an admin disabling the user or node
        // either sees this connection in the registry or we see their change.
        let conn = ctx.store.register_conn(&user.username, &node.id, target.to_string(), peer);
        if !ctx.store.still_allowed(&user, &node) {
            reply_error(&mut client, rep::NOT_ALLOWED).await;
            return Ok(());
        }
        let _node_slot = node.stats.enter();

        let opened = tokio::select! {
            r = open_stream(&session, &target, deadline) => r,
            _ = conn.entry.cancel.cancelled() => {
                reply_error(&mut client, rep::NOT_ALLOWED).await;
                return Ok(());
            }
        };
        let (mut data, bound) = match opened {
            Ok(x) => x,
            // The node went away mid-request: try another one.
            Err(ConnectError::SessionGone) if Instant::now() < deadline => continue,
            Err(e) => {
                let (code, err) = match e {
                    ConnectError::Refused(code) => (code, io::Error::other(format!("node could not reach {target}"))),
                    ConnectError::SessionGone => (rep::NETWORK_UNREACHABLE, io::Error::other("node went offline")),
                    ConnectError::TimedOut => {
                        (rep::TTL_EXPIRED, io::Error::new(io::ErrorKind::TimedOut, "connect timed out"))
                    }
                    ConnectError::Io(e) => (rep::GENERAL_FAILURE, e),
                };
                reply_error(&mut client, code).await;
                return Err(err);
            }
        };

        // Count before replying, so the client never sees a connection the stats don't.
        user.stats.total.fetch_add(1, Ordering::Relaxed);
        node.stats.total.fetch_add(1, Ordering::Relaxed);

        let mut reply = vec![SOCKS_VERSION, rep::SUCCEEDED, 0];
        bound.encode(&mut reply);
        client.write_all(&reply).await?;
        client.flush().await?;
        debug!(id = conn.entry.id, user = %user.username, node = %node.id, %target, "stream open");

        let mut metered =
            Metered { inner: client, conn: conn.entry.clone(), user: user.stats.clone(), node: node.stats.clone() };
        return tokio::select! {
            r = relay(&mut metered, &mut data) => r.map(drop),
            _ = conn.entry.cancel.cancelled() => Ok(()),
        };
    }
}

/// Have the node connect to `target` over one of its pooled tunnel connections.
async fn open_stream(
    session: &NodeSession,
    target: &Target,
    deadline: Instant,
) -> Result<(TunnelConn, Target), ConnectError> {
    let mut last_err = None;
    for _ in 0..STALE_RETRIES {
        let Some(mut conn) = session.acquire(deadline).await else {
            return Err(if session.cancel.is_cancelled() { ConnectError::SessionGone } else { ConnectError::TimedOut });
        };
        let exchange = async {
            proto::write_connect(&mut conn, target).await?;
            proto::read_reply(&mut conn).await
        };
        match tokio::time::timeout_at(deadline, exchange).await {
            Ok(Ok((rep::SUCCEEDED, bound))) => return Ok((conn, bound)),
            Ok(Ok((code, _))) => return Err(ConnectError::Refused(code)),
            // Most likely a stale pooled connection; try a fresh one. The node
            // never got far enough to send data to the target, so this is safe.
            Ok(Err(e)) => last_err = Some(e),
            Err(_) => return Err(ConnectError::TimedOut),
        }
    }
    Err(ConnectError::Io(last_err.unwrap_or_else(|| io::Error::other("no usable tunnel connection"))))
}

async fn handshake<S>(s: &mut S, peer: SocketAddr, ctx: &SocksCtx) -> io::Result<(Arc<UserEntry>, Target)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Greeting: VER NMETHODS METHODS...
    let mut head = [0u8; 2];
    s.read_exact(&mut head).await?;
    if head[0] != SOCKS_VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a SOCKS5 client"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    s.read_exact(&mut methods).await?;
    if !methods.contains(&METHOD_USERPASS) {
        s.write_all(&[SOCKS_VERSION, METHOD_NONE_ACCEPTABLE]).await?;
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "client doesn't offer username/password auth"));
    }
    s.write_all(&[SOCKS_VERSION, METHOD_USERPASS]).await?;
    s.flush().await?;

    // RFC 1929: VER ULEN UNAME PLEN PASSWD
    if s.read_u8().await? != USERPASS_VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad username/password auth version"));
    }
    let username = read_short_string(s).await?;
    let password = read_short_string(s).await?;
    if ctx.limiter.is_blocked(peer.ip(), &username) {
        s.write_all(&[USERPASS_VERSION, AUTH_FAILED]).await?;
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "too many failed logins"));
    }
    let Some(user) = ctx.store.authenticate(&username, &password) else {
        ctx.limiter.record_failure(peer.ip(), &username);
        warn!(%peer, user = ?username, "socks authentication failed");
        s.write_all(&[USERPASS_VERSION, AUTH_FAILED]).await?;
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "authentication failed"));
    };
    ctx.limiter.record_success(peer.ip(), &username);
    s.write_all(&[USERPASS_VERSION, AUTH_OK]).await?;
    s.flush().await?;

    // Request: VER CMD RSV DST
    let mut req = [0u8; 3];
    s.read_exact(&mut req).await?;
    if req[0] != SOCKS_VERSION {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad request version"));
    }
    let target = match Target::read_from(s).await {
        Ok(t) => t,
        Err(e) => {
            reply_error(s, rep::ADDRESS_NOT_SUPPORTED).await;
            return Err(e);
        }
    };
    if req[1] != CMD_CONNECT {
        reply_error(s, rep::COMMAND_NOT_SUPPORTED).await;
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("unsupported SOCKS command {}", req[1])));
    }
    Ok((user, target))
}

async fn reply_error<S: AsyncWrite + Unpin>(s: &mut S, code: u8) {
    let mut reply = vec![SOCKS_VERSION, code, 0];
    Target::unspecified().encode(&mut reply);
    let _ = s.write_all(&reply).await;
    let _ = s.flush().await;
}

/// Wraps the client stream and adds what passes through to the connection,
/// user and node counters.
struct Metered<S> {
    inner: S,
    conn: Arc<ConnEntry>,
    user: Arc<Traffic>,
    node: Arc<Traffic>,
}

impl<S: AsyncRead + Unpin> AsyncRead for Metered<S> {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = (buf.filled().len() - before) as u64;
        if n > 0 {
            self.conn.up.fetch_add(n, Ordering::Relaxed);
            self.user.up.fetch_add(n, Ordering::Relaxed);
            self.node.up.fetch_add(n, Ordering::Relaxed);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Metered<S> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = result {
            let n = n as u64;
            self.conn.down.fetch_add(n, Ordering::Relaxed);
            self.user.down.fetch_add(n, Ordering::Relaxed);
            self.node.down.fetch_add(n, Ordering::Relaxed);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::balance::Strategy;
    use crate::settings::Settings;
    use crate::state::{SocksPassword, UserCfg};

    fn ctx() -> SocksCtx {
        let store = Store::load(std::env::temp_dir().join("wirefox-never-written.json")).unwrap();
        let cfg = UserCfg {
            password: SocksPassword::new("secret"),
            enabled: true,
            nodes: vec![],
            max_connections: None,
            note: None,
            balance: None,
            created_at: 0,
        };
        store.insert_user("alice".into(), cfg);
        SocksCtx {
            store,
            limiter: Arc::new(SocksAuthLimiter::new(Duration::from_secs(60))),
            settings: Arc::new(LiveSettings::new(Settings {
                balance: Strategy::default(),
                handshake_timeout_secs: 5,
                connect_timeout_secs: 5,
            })),
        }
    }

    fn peer() -> SocketAddr {
        "192.0.2.1:5000".parse().unwrap()
    }

    fn auth(user: &str, pass: &str) -> Vec<u8> {
        let mut v = vec![5, 1, 2, 1, user.len() as u8];
        v.extend_from_slice(user.as_bytes());
        v.push(pass.len() as u8);
        v.extend_from_slice(pass.as_bytes());
        v
    }

    async fn run_handshake(ctx: &SocksCtx, input: Vec<u8>) -> (io::Result<Target>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(&input).await.unwrap();
        let result = handshake(&mut server, peer(), ctx).await.map(|(_, t)| t);
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        (result, out)
    }

    #[tokio::test]
    async fn successful_connect_request() {
        let mut input = auth("alice", "secret");
        input.extend_from_slice(&[5, 1, 0, 3, 11]);
        input.extend_from_slice(b"example.com");
        input.extend_from_slice(&443u16.to_be_bytes());
        let (result, out) = run_handshake(&ctx(), input).await;
        assert_eq!(result.unwrap().to_string(), "example.com:443");
        assert_eq!(out, [5, 2, 1, 0]);
    }

    #[tokio::test]
    async fn wrong_password_is_rejected_and_counted() {
        let ctx = ctx();
        for _ in 0..10 {
            let (result, out) = run_handshake(&ctx, auth("alice", "wrong")).await;
            assert!(result.is_err());
            assert_eq!(out, [5, 2, 1, 1]);
        }
        // Blocked now, even with the right password.
        let (result, _) = run_handshake(&ctx, auth("alice", "secret")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn no_auth_offered() {
        let (result, out) = run_handshake(&ctx(), vec![5, 1, 0]).await;
        assert!(result.is_err());
        assert_eq!(out, [5, 0xFF]);
    }

    #[tokio::test]
    async fn udp_associate_not_supported() {
        let mut input = auth("alice", "secret");
        input.extend_from_slice(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0]);
        let (result, out) = run_handshake(&ctx(), input).await;
        assert!(result.is_err());
        assert_eq!(out[4..6], [5, rep::COMMAND_NOT_SUPPORTED]);
    }
}
