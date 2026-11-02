//! The tunnel listener. Nodes connect here with one control connection and a
//! pool of idle data connections; each SOCKS stream takes one from the pool.

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use wirefox_common::net::{accept, tune_tcp};
use wirefox_common::proto::{
    self, hello_status, ConnKind, EgressSpec, Hello, NodeInfo, NodeMsg, ServerMsg, CONTROL_IDLE_TIMEOUT, PING_INTERVAL,
};

use crate::state::{now_secs, NodeEntry, Store};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// While a request waits for a data connection, ask the node again this often.
const DEMAND_RETRY: Duration = Duration::from_secs(1);
/// Cap on idle data connections a single node may park here.
const MAX_IDLE_PER_NODE: usize = 4096;

pub type TunnelConn = TlsStream<TcpStream>;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// A connected exit-node.
pub struct NodeSession {
    pub id: u64,
    pub remote: SocketAddr,
    pub connected_at: u64,
    pub info: RwLock<NodeInfo>,
    /// Egress the node reports as active, and the error from the last override, if any.
    pub egress_state: RwLock<(EgressSpec, Option<String>)>,
    pub rtt_us: AtomicU64,
    tx: mpsc::Sender<ServerMsg>,
    pool: Mutex<VecDeque<TunnelConn>>,
    pool_ready: Notify,
    pub cancel: CancellationToken,
}

#[derive(Serialize)]
pub struct SessionSnapshot {
    pub session_id: u64,
    pub remote: SocketAddr,
    pub connected_at: u64,
    pub rtt_ms: f64,
    pub idle_pool: usize,
    pub info: NodeInfo,
    pub active_egress: EgressSpec,
    pub egress_error: Option<String>,
}

impl NodeSession {
    /// Queue a control message. Never blocks; drops the message if the queue is full.
    pub fn send(&self, msg: ServerMsg) -> bool {
        self.tx.try_send(msg).is_ok()
    }

    fn push(&self, conn: TunnelConn) {
        self.pool.lock().push_back(conn);
        self.pool_ready.notify_one();
    }

    /// Take an idle data connection, asking the node for more while the pool is empty.
    /// Returns `None` at `deadline` or when the session ends.
    pub async fn acquire(&self, deadline: tokio::time::Instant) -> Option<TunnelConn> {
        let mut next_demand = tokio::time::Instant::now();
        loop {
            // Register interest before looking, so a push in between isn't missed.
            let ready = self.pool_ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(conn) = self.take_idle() {
                return Some(conn);
            }
            if self.cancel.is_cancelled() {
                return None;
            }
            if tokio::time::Instant::now() >= next_demand {
                self.send(ServerMsg::Demand { count: 1 });
                next_demand = tokio::time::Instant::now() + DEMAND_RETRY;
            }
            tokio::select! {
                _ = &mut ready => {}
                _ = tokio::time::sleep_until(next_demand.min(deadline)) => {
                    if tokio::time::Instant::now() >= deadline {
                        return None;
                    }
                }
                _ = self.cancel.cancelled() => return None,
            }
        }
    }

    fn take_idle(&self) -> Option<TunnelConn> {
        let mut pool = self.pool.lock();
        // Newest first: the most recently opened connection is the least likely
        // to have been dropped by a NAT box along the way.
        while let Some(mut conn) = pool.pop_back() {
            if is_idle(&mut conn) {
                if !pool.is_empty() {
                    // Pass the wakeup on in case several requests are waiting.
                    self.pool_ready.notify_one();
                }
                return Some(conn);
            }
        }
        None
    }

    fn prune_pool(&self) {
        self.pool.lock().retain_mut(is_idle);
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let (active_egress, egress_error) = self.egress_state.read().clone();
        SessionSnapshot {
            session_id: self.id,
            remote: self.remote,
            connected_at: self.connected_at,
            rtt_ms: self.rtt_us.load(Ordering::Relaxed) as f64 / 1000.0,
            idle_pool: self.pool.lock().len(),
            info: self.info.read().clone(),
            active_egress,
            egress_error,
        }
    }
}

/// The node never sends anything on an idle data connection, so if it is
/// readable at all (data, EOF or an error) it is unusable.
fn is_idle<S: AsyncRead + Unpin>(conn: &mut S) -> bool {
    let mut cx = Context::from_waker(Waker::noop());
    let mut byte = [0u8; 1];
    let mut buf = ReadBuf::new(&mut byte);
    matches!(Pin::new(conn).poll_read(&mut cx, &mut buf), Poll::Pending)
}

pub async fn serve(listener: TcpListener, acceptor: TlsAcceptor, store: Arc<Store>) {
    loop {
        let (stream, peer) = accept(&listener).await;
        let acceptor = acceptor.clone();
        let store = store.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, peer, acceptor, store).await {
                debug!(%peer, error = %e, "tunnel connection closed");
            }
        });
    }
}

async fn handle(stream: TcpStream, peer: SocketAddr, acceptor: TlsAcceptor, store: Arc<Store>) -> io::Result<()> {
    tune_tcp(&stream)?;
    let mut tls = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "TLS handshake timed out"))??;

    let hello = match tokio::time::timeout(HANDSHAKE_TIMEOUT, Hello::read_from(&mut tls)).await {
        Ok(Ok(hello)) => hello,
        Ok(Err(rejected)) => {
            let _ = tls.write_all(&[rejected.status]).await;
            return Err(rejected.error);
        }
        Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "hello timed out")),
    };

    let node = match store.node(&hello.node_id) {
        Some(node) if node.cfg.read().token_matches(&hello.token) => node,
        _ => {
            warn!(%peer, node = ?hello.node_id, "tunnel authentication failed");
            let _ = tls.write_all(&[hello_status::BAD_AUTH]).await;
            return Ok(());
        }
    };
    if !node.cfg.read().enabled {
        let _ = tls.write_all(&[hello_status::DISABLED]).await;
        return Ok(());
    }

    match hello.kind {
        ConnKind::Data => {
            let Some(session) = node.online_session() else {
                let _ = tls.write_all(&[hello_status::NO_SESSION]).await;
                return Ok(());
            };
            if session.pool.lock().len() >= MAX_IDLE_PER_NODE {
                let _ = tls.write_all(&[hello_status::POOL_FULL]).await;
                return Ok(());
            }
            tls.write_all(&[hello_status::OK]).await?;
            tls.flush().await?;
            session.push(tls);
            Ok(())
        }
        ConnKind::Control => {
            tls.write_all(&[hello_status::OK]).await?;
            tls.flush().await?;
            run_control(&store, node, tls, peer, &hello.token).await
        }
    }
}

async fn run_control(
    store: &Store,
    node: Arc<NodeEntry>,
    tls: TunnelConn,
    peer: SocketAddr,
    token: &str,
) -> io::Result<()> {
    let (mut rd, wr) = tokio::io::split(tls);
    let info = match tokio::time::timeout(HANDSHAKE_TIMEOUT, proto::read_frame::<_, NodeMsg>(&mut rd)).await {
        Ok(Ok(NodeMsg::Info(info))) => info,
        Ok(Ok(_)) => return Err(io::Error::new(io::ErrorKind::InvalidData, "expected node info first")),
        Ok(Err(e)) => return Err(e),
        Err(_) => return Err(io::Error::new(io::ErrorKind::TimedOut, "node info timed out")),
    };

    let (tx, rx) = mpsc::channel(256);
    let session = Arc::new(NodeSession {
        id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
        remote: peer,
        connected_at: now_secs(),
        egress_state: RwLock::new((info.active_egress.clone(), None)),
        info: RwLock::new(info),
        rtt_us: AtomicU64::new(0),
        tx,
        pool: Mutex::new(VecDeque::new()),
        pool_ready: Notify::new(),
        cancel: CancellationToken::new(),
    });

    // Install the session, unless the node was deleted, disabled or had its token
    // rotated since the hello. Lock order everywhere: store.nodes, node.cfg, node.session.
    {
        let nodes = store.nodes.read();
        let cfg = node.cfg.read();
        let mut slot = node.session.write();
        let still_valid =
            nodes.get(&node.id).is_some_and(|n| Arc::ptr_eq(n, &node)) && cfg.enabled && cfg.token_matches(token);
        if !still_valid {
            return Ok(());
        }
        if let Some(old) = slot.replace(session.clone()) {
            old.cancel.cancel();
        }
        // Always send the desired egress: `None` clears an override the node may
        // still hold from before an outage. This is also the node's cue to start
        // its pool.
        session.send(ServerMsg::SetEgress { egress: cfg.egress.clone() });
    }
    {
        let i = session.info.read();
        info!(node = %node.id, %peer, host = %i.hostname, os = %i.os, version = %i.version, "node connected");
    }

    let result = tokio::select! {
        r = write_control(wr, rx, &session) => r,
        r = read_control(&mut rd, &node, &session) => r,
        _ = session.cancel.cancelled() => Ok(()),
    };

    session.cancel.cancel();
    {
        let mut slot = node.session.write();
        if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &session)) {
            *slot = None;
        }
    }
    session.pool.lock().clear();
    info!(node = %node.id, %peer, "node disconnected");
    result
}

async fn write_control(
    mut wr: WriteHalf<TunnelConn>,
    mut rx: mpsc::Receiver<ServerMsg>,
    session: &NodeSession,
) -> io::Result<()> {
    let mut ping = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(msg) => proto::write_frame(&mut wr, &msg).await?,
                None => return Ok(()),
            },
            _ = ping.tick() => {
                // The node echoes `seq` back, so a timestamp gives us the RTT for free.
                proto::write_frame(&mut wr, &ServerMsg::Ping { seq: monotonic_us() }).await?;
                session.prune_pool();
            }
        }
    }
}

fn monotonic_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

async fn read_control(rd: &mut ReadHalf<TunnelConn>, node: &NodeEntry, session: &NodeSession) -> io::Result<()> {
    loop {
        let msg = tokio::time::timeout(CONTROL_IDLE_TIMEOUT, proto::read_frame(rd))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control connection went silent"))??;
        match msg {
            NodeMsg::Pong { seq } => {
                let rtt = monotonic_us().saturating_sub(seq);
                session.rtt_us.store(rtt, Ordering::Relaxed);
            }
            NodeMsg::Info(info) => {
                session.egress_state.write().0 = info.active_egress.clone();
                *session.info.write() = info;
            }
            NodeMsg::Interfaces { interfaces } => session.info.write().interfaces = interfaces,
            NodeMsg::EgressApplied { egress, error } => {
                if let Some(e) = &error {
                    warn!(node = %node.id, error = %e, "node rejected its egress setting");
                }
                session.info.write().active_egress = egress.clone();
                *session.egress_state.write() = (egress, error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt;

    use super::is_idle;

    #[tokio::test]
    async fn idle_detection() {
        let (mut near, mut far) = tokio::io::duplex(64);
        assert!(is_idle(&mut near));
        far.write_all(b"x").await.unwrap();
        assert!(!is_idle(&mut near), "unexpected data means the connection is unusable");

        let (mut near, far) = tokio::io::duplex(64);
        drop(far);
        assert!(!is_idle(&mut near), "a closed connection is not idle");
    }
}
