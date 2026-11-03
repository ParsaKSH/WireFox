//! The reverse tunnel: one control connection plus a pool of idle data
//! connections, all dialed from the node to the server.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use parking_lot::RwLock;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch, Semaphore};
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::TlsConnector;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use wirefox_common::net::{relay, tune_tcp};
use wirefox_common::proto::{
    self, hello_status, rep, ConnKind, Hello, Host, NodeInfo, NodeMsg, ServerMsg, Target, CONTROL_IDLE_TIMEOUT,
};

use crate::config::NodeConfig;
use crate::egress::{list_interfaces, Dialer};

/// On-demand connections the server doesn't claim within this time are closed.
const EXTRA_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_DEMAND_PER_MSG: u32 = 256;
const MAX_BACKOFF: Duration = Duration::from_secs(10);

type Conn = TlsStream<TcpStream>;

/// The server answered our hello with something other than OK.
#[derive(Debug)]
struct Rejected(u8);

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "server rejected the connection: {}", hello_status::describe(self.0))
    }
}

impl std::error::Error for Rejected {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TunnelState {
    Stopped,
    Connecting,
    Connected,
    /// The last attempt failed and the node keeps retrying.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TunnelStatus {
    pub state: TunnelState,
    /// Why the tunnel is down, while it is in [`TunnelState::Error`].
    pub last_error: Option<String>,
    pub connected_since: Option<Instant>,
}

/// Observable state of a tunnel. A monitor can be shared by consecutive nodes
/// (managed mode replaces the node on every config change), so its stream
/// counters cover the whole process.
pub struct Monitor {
    status: watch::Sender<TunnelStatus>,
    active_streams: Arc<AtomicU64>,
    total_streams: AtomicU64,
}

impl Default for Monitor {
    fn default() -> Self {
        let status = TunnelStatus { state: TunnelState::Stopped, last_error: None, connected_since: None };
        Monitor {
            status: watch::Sender::new(status),
            active_streams: Arc::new(AtomicU64::new(0)),
            total_streams: AtomicU64::new(0),
        }
    }
}

impl Monitor {
    pub fn status(&self) -> TunnelStatus {
        self.status.borrow().clone()
    }

    /// Notifies on every change of the tunnel status.
    pub fn subscribe(&self) -> watch::Receiver<TunnelStatus> {
        self.status.subscribe()
    }

    /// Streams being relayed right now.
    pub fn active_streams(&self) -> u64 {
        self.active_streams.load(Ordering::Relaxed)
    }

    /// Streams relayed since the monitor was created.
    pub fn total_streams(&self) -> u64 {
        self.total_streams.load(Ordering::Relaxed)
    }

    /// For the owner of a node that has been shut down.
    pub fn set_stopped(&self) {
        self.set(TunnelState::Stopped, None);
    }

    fn set(&self, state: TunnelState, last_error: Option<String>) {
        self.status.send_if_modified(|current| {
            if current.state == state && current.last_error == last_error {
                return false;
            }
            let connected_since = (state == TunnelState::Connected).then(Instant::now);
            *current = TunnelStatus { state, last_error, connected_since };
            true
        });
    }

    fn stream_started(&self) -> ActiveStream {
        self.total_streams.fetch_add(1, Ordering::Relaxed);
        self.active_streams.fetch_add(1, Ordering::Relaxed);
        ActiveStream(self.active_streams.clone())
    }
}

/// Counts as an active stream until dropped.
struct ActiveStream(Arc<AtomicU64>);

impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct Node {
    cfg: NodeConfig,
    connector: TlsConnector,
    server_name: ServerName<'static>,
    server: Target,
    /// Server addresses resolved when the control session was set up, so data
    /// connections don't each pay for a DNS lookup.
    server_addrs: RwLock<Vec<SocketAddr>>,
    /// Dialer for proxied traffic.
    egress: Dialer,
    /// Dialer for connections to the server.
    uplink: Dialer,
    extra: Arc<Semaphore>,
    monitor: Arc<Monitor>,
}

impl Node {
    pub fn new(cfg: NodeConfig) -> Result<Arc<Self>> {
        Self::with_monitor(cfg, Arc::default())
    }

    /// A node that reports its state to `monitor`.
    pub fn with_monitor(cfg: NodeConfig, monitor: Arc<Monitor>) -> Result<Arc<Self>> {
        let tls = wirefox_common::tls::pinned_client_config(&cfg.server_fingerprint)?;
        let server_name = ServerName::try_from(cfg.server_name.clone()).context("invalid server_name")?;
        let server = parse_host_port(&cfg.server)?;
        let timeout = Duration::from_secs(cfg.connect_timeout_secs);
        Ok(Arc::new(Node {
            egress: Dialer::new(cfg.egress.clone(), timeout, cfg.allow_local),
            uplink: Dialer::new(cfg.uplink.clone(), timeout, true),
            extra: Arc::new(Semaphore::new(cfg.max_extra as usize)),
            server_addrs: RwLock::new(Vec::new()),
            connector: TlsConnector::from(tls),
            server_name,
            server,
            cfg,
            monitor,
        }))
    }

    pub fn monitor(&self) -> &Arc<Monitor> {
        &self.monitor
    }

    /// Why proxied traffic can't leave right now, if it can't: the egress
    /// interface is missing, or the source address isn't on it. The tunnel
    /// itself may well be up meanwhile.
    pub fn egress_error(&self) -> Option<String> {
        self.egress.error().map(|e| format!("egress: {e}"))
    }

    /// Stay connected to the server forever, reconnecting with backoff.
    pub async fn run(self: Arc<Self>) {
        self.monitor.set(TunnelState::Connecting, None);
        let mut backoff = Duration::from_secs(1);
        loop {
            let started = Instant::now();
            match self.clone().session().await {
                Ok(()) => {
                    info!("control session closed by the server");
                    self.monitor.set(TunnelState::Connecting, None);
                }
                Err(e) => {
                    warn!("control session ended: {e:#}");
                    self.monitor.set(TunnelState::Error, Some(format!("{e:#}")));
                }
            }
            if started.elapsed() > Duration::from_secs(60) {
                backoff = Duration::from_secs(1);
            }
            let jitter = Duration::from_millis(rand::random::<u64>() % 500);
            tokio::time::sleep(backoff + jitter).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn connect(&self, kind: ConnKind, addrs: Vec<SocketAddr>) -> Result<Conn> {
        let tcp = self.uplink.dial_addrs(addrs).await.context("connecting to the server")?;
        tune_tcp(&tcp)?;
        let mut tls = tokio::time::timeout(
            Duration::from_secs(self.cfg.connect_timeout_secs),
            self.connector.connect(self.server_name.clone(), tcp),
        )
        .await
        .context("TLS handshake timed out")?
        .context("TLS handshake")?;
        Hello { kind, node_id: self.cfg.node_id.clone(), token: self.cfg.token.clone() }.write_to(&mut tls).await?;
        match tls.read_u8().await.context("reading the hello reply")? {
            hello_status::OK => Ok(tls),
            status => Err(Rejected(status).into()),
        }
    }

    async fn open_data(&self) -> Result<Conn> {
        let addrs = self.server_addrs.read().clone();
        self.connect(ConnKind::Data, addrs).await
    }

    async fn session(self: Arc<Self>) -> Result<()> {
        let addrs =
            self.uplink.resolve(&self.server).await.with_context(|| format!("resolving {}", self.cfg.server))?;
        let mut ctrl = tokio::time::timeout(
            Duration::from_secs(self.cfg.connect_timeout_secs) * 2,
            self.connect(ConnKind::Control, addrs.clone()),
        )
        .await
        .context("connecting to the server timed out")??;
        // Data connections go to the address that just worked, then the rest.
        let peer = ctrl.get_ref().0.peer_addr()?;
        *self.server_addrs.write() = std::iter::once(peer).chain(addrs.into_iter().filter(|a| *a != peer)).collect();
        info!(server = %self.cfg.server, %peer, "connected to server");
        self.monitor.set(TunnelState::Connected, None);

        let info = NodeInfo {
            hostname: hostname(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            version: wirefox_common::VERSION.into(),
            pool_size: self.cfg.pool_size,
            local_egress: self.egress.local_spec().clone(),
            active_egress: self.egress.effective(),
            interfaces: list_interfaces(),
        };
        proto::write_frame(&mut ctrl, &NodeMsg::Info(info)).await?;

        let (rd, wr) = tokio::io::split(ctrl);
        let (tx, rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let _stop_pool = cancel.clone().drop_guard();
        tokio::select! {
            r = write_control(wr, rx) => r?,
            r = self.read_control(rd, tx, &cancel) => r?,
            _ = cancel.cancelled() => return Err(anyhow!("the server no longer accepts this node's data connections")),
        }
        Ok(())
    }

    async fn read_control(
        self: &Arc<Self>,
        mut rd: ReadHalf<Conn>,
        tx: mpsc::Sender<NodeMsg>,
        cancel: &CancellationToken,
    ) -> io::Result<()> {
        let mut pool_started = false;
        loop {
            let msg: ServerMsg = match tokio::time::timeout(CONTROL_IDLE_TIMEOUT, proto::read_frame(&mut rd)).await {
                Ok(Ok(msg)) => msg,
                // The server closed the connection, e.g. because it is restarting.
                Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "control connection went silent")),
            };
            // The server only starts talking once our session is registered, so
            // data connections opened from here on will be accepted.
            if !pool_started {
                pool_started = true;
                for _ in 0..self.cfg.pool_size {
                    tokio::spawn(self.clone().pool_slot(cancel.clone()));
                }
            }
            let reply = match msg {
                ServerMsg::Ping { seq } => Some(NodeMsg::Pong { seq }),
                ServerMsg::Demand { count } => {
                    self.spawn_extra(count.min(MAX_DEMAND_PER_MSG), cancel);
                    None
                }
                ServerMsg::SetEgress { egress } => {
                    let error = self.egress.set_override(egress);
                    let egress = self.egress.effective();
                    match &error {
                        None => info!(%egress, "egress set by server"),
                        Some(e) => warn!(%egress, error = %e, "egress set by server is not usable yet"),
                    }
                    Some(NodeMsg::EgressApplied { egress, error })
                }
                ServerMsg::RefreshInterfaces => Some(NodeMsg::Interfaces { interfaces: list_interfaces() }),
            };
            if let Some(reply) = reply {
                if tx.send(reply).await.is_err() {
                    return Ok(());
                }
            }
        }
    }

    /// Keep one idle data connection open at all times. As soon as the server
    /// hands it a stream, serve that in the background and open a replacement.
    async fn pool_slot(self: Arc<Self>, cancel: CancellationToken) {
        loop {
            let conn = tokio::select! {
                _ = cancel.cancelled() => return,
                r = self.open_data() => r,
            };
            let mut conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    if let Some(Rejected(hello_status::BAD_AUTH | hello_status::DISABLED)) = e.downcast_ref() {
                        // Token rotated or node disabled: drop the whole session.
                        cancel.cancel();
                        return;
                    }
                    debug!("data connection failed: {e:#}");
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(Duration::from_secs(1)) => continue,
                    }
                }
            };
            let request = tokio::select! {
                _ = cancel.cancelled() => return,
                r = proto::read_connect(&mut conn) => r,
            };
            match request {
                Ok(target) => {
                    tokio::spawn(self.clone().serve(conn, target));
                }
                Err(e) => debug!(error = %e, "idle data connection closed"),
            }
        }
    }

    /// Open up to `count` more data connections for a burst. Like the pool
    /// slots they end with the session (and so with the node), unless they
    /// already carry a stream: a node that was replaced must not pick up
    /// streams for its successor.
    fn spawn_extra(self: &Arc<Self>, count: u32, cancel: &CancellationToken) {
        for _ in 0..count {
            let Ok(permit) = self.extra.clone().try_acquire_owned() else {
                debug!("max_extra reached, ignoring the rest of the demand");
                return;
            };
            let (node, cancel) = (self.clone(), cancel.clone());
            tokio::spawn(async move {
                let conn = tokio::select! {
                    _ = cancel.cancelled() => return,
                    r = node.open_data() => r,
                };
                let mut conn = match conn {
                    Ok(c) => c,
                    Err(e) => return debug!("extra data connection failed: {e:#}"),
                };
                let request = tokio::select! {
                    _ = cancel.cancelled() => None,
                    r = tokio::time::timeout(EXTRA_IDLE_TIMEOUT, proto::read_connect(&mut conn)) => r.ok(),
                };
                drop(permit);
                match request {
                    Some(Ok(target)) => node.serve(conn, target).await,
                    // Close properly so the server drops it from its pool right away.
                    _ => {
                        let _ = conn.shutdown().await;
                    }
                }
            });
        }
    }

    async fn serve(self: Arc<Self>, mut conn: Conn, target: Target) {
        let mut out = match self.egress.dial(&target).await {
            Ok(s) => s,
            Err(e) => {
                debug!(%target, error = %e, "dial failed");
                let _ = proto::write_reply(&mut conn, rep::from_io_error(&e), &Target::unspecified()).await;
                let _ = conn.shutdown().await;
                return;
            }
        };
        let bound = out.local_addr().map(Target::from_socket_addr).unwrap_or_else(|_| Target::unspecified());
        let _active = self.monitor.stream_started();
        if proto::write_reply(&mut conn, rep::SUCCEEDED, &bound).await.is_err() {
            return;
        }
        match relay(&mut conn, &mut out).await {
            Ok((up, down)) => debug!(%target, up, down, "stream closed"),
            Err(e) => debug!(%target, error = %e, "stream ended with an error"),
        }
    }
}

async fn write_control(mut wr: WriteHalf<Conn>, mut rx: mpsc::Receiver<NodeMsg>) -> io::Result<()> {
    while let Some(msg) = rx.recv().await {
        proto::write_frame(&mut wr, &msg).await?;
    }
    Ok(())
}

pub(crate) fn parse_host_port(s: &str) -> Result<Target> {
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(Target::from_socket_addr(addr));
    }
    let (host, port) = s.rsplit_once(':').context("server must be host:port")?;
    anyhow::ensure!(!host.is_empty() && !host.contains(':'), "server must be host:port (IPv6 needs [brackets])");
    let port = port.parse().context("invalid server port")?;
    Ok(Target { host: Host::Domain(host.to_string()), port })
}

pub(crate) fn hostname() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(not(windows))]
    let name = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok());
    name.map(|s| s.trim().to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_parsing() {
        assert_eq!(parse_host_port("1.2.3.4:7443").unwrap(), Target::from_socket_addr("1.2.3.4:7443".parse().unwrap()));
        assert_eq!(parse_host_port("[::1]:7443").unwrap(), Target::from_socket_addr("[::1]:7443".parse().unwrap()));
        assert_eq!(
            parse_host_port("relay.example.com:7443").unwrap(),
            Target { host: Host::Domain("relay.example.com".into()), port: 7443 }
        );
        for bad in ["relay.example.com", "relay.example.com:http", ":7443", "::1:7443", "host:70000"] {
            assert!(parse_host_port(bad).is_err(), "{bad}");
        }
    }
}
