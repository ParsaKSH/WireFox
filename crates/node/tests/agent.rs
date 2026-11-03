//! Managed mode against an in-process panel: a real tonic Agent server behind
//! TLS with a self-signed certificate, which the node pins through its key.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer;
use tokio_rustls::TlsAcceptor;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing_subscriber::layer::SubscriberExt;
use wirefox_common::tls::CertKey;
use wirefox_node::managed::{LogBuffer, Managed, NodeKey, StateDir};
use wirefox_node::pb::agent_server::{Agent, AgentServer};
use wirefox_node::pb::{
    command, command_result, node_message, panel_message, Command, CommandResult, Egress, FetchLogs, Hello,
    ListInterfaces, NodeMessage, PanelMessage, TunnelConfig, TunnelState, Welcome,
};

const WAIT: Duration = Duration::from_secs(15);

/// One accepted Connect stream, driven by the test.
struct Session {
    inbound: Streaming<NodeMessage>,
    outbound: mpsc::Sender<Result<PanelMessage, Status>>,
}

impl Session {
    async fn send(&self, msg: panel_message::Msg) {
        self.outbound.send(Ok(PanelMessage { msg: Some(msg) })).await.unwrap();
    }

    async fn recv(&mut self) -> node_message::Msg {
        let msg = tokio::time::timeout(WAIT, self.inbound.message()).await.expect("node went silent");
        msg.unwrap().expect("node closed the stream").msg.unwrap()
    }

    async fn hello(&mut self) -> Hello {
        match self.recv().await {
            node_message::Msg::Hello(hello) => hello,
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    /// Next command result, skipping status reports.
    async fn result(&mut self) -> CommandResult {
        loop {
            if let node_message::Msg::Result(result) = self.recv().await {
                return result;
            }
        }
    }
}

struct TestPanel {
    secret: String,
    sessions: mpsc::UnboundedSender<Session>,
    rejected: mpsc::UnboundedSender<()>,
}

#[tonic::async_trait]
impl Agent for TestPanel {
    type ConnectStream = ReceiverStream<Result<PanelMessage, Status>>;

    async fn connect(&self, request: Request<Streaming<NodeMessage>>) -> Result<Response<Self::ConnectStream>, Status> {
        let auth = request.metadata().get("authorization").and_then(|v| v.to_str().ok());
        if auth != Some(format!("Bearer {}", self.secret).as_str()) {
            let _ = self.rejected.send(());
            return Err(Status::unauthenticated("unknown node key"));
        }
        let (tx, rx) = mpsc::channel(16);
        let _ = self.sessions.send(Session { inbound: request.into_inner(), outbound: tx });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

struct Panel {
    addr: SocketAddr,
    fingerprint: String,
    secret: String,
    sessions: mpsc::UnboundedReceiver<Session>,
    rejected: mpsc::UnboundedReceiver<()>,
    shutdown: oneshot::Sender<()>,
    server: JoinHandle<()>,
}

impl Panel {
    /// Serve on `addr` (port 0 for any) with a certificate made for the "wirefox-panel" SNI.
    async fn start(addr: SocketAddr, cert: &CertKey) -> Panel {
        let mut tls = Arc::unwrap_or_clone(wirefox_common::tls::server_config(cert, true).unwrap());
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = wirefox_common::net::bind_listener(addr).unwrap();
        let addr = listener.local_addr().unwrap();

        let (conn_tx, conn_rx) = mpsc::channel(4);
        let accept = tokio::spawn(async move {
            loop {
                let (tcp, _) = wirefox_common::net::accept(&listener).await;
                let Ok(tls) = acceptor.accept(tcp).await else { continue };
                assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
                if conn_tx.send(Ok::<_, std::io::Error>(tls)).await.is_err() {
                    return;
                }
            }
        });

        let secret = "5e".repeat(32);
        let (sessions_tx, sessions) = mpsc::unbounded_channel();
        let (rejected_tx, rejected) = mpsc::unbounded_channel();
        let service =
            AgentServer::new(TestPanel { secret: secret.clone(), sessions: sessions_tx, rejected: rejected_tx });
        let (shutdown, stop) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .serve_with_incoming_shutdown(service, ReceiverStream::new(conn_rx), async {
                    let _ = stop.await;
                })
                .await
                .unwrap();
            // The listener goes with the accept loop, freeing the port.
            accept.abort();
            let _ = accept.await;
        });
        Panel { addr, fingerprint: cert.fingerprint(), secret, sessions, rejected, shutdown, server }
    }

    fn key(&self, secret: &str) -> NodeKey {
        let json = serde_json::json!({ "panel": self.addr.to_string(), "fp": self.fingerprint, "secret": secret });
        NodeKey::parse(&format!("wfx1.{}", URL_SAFE_NO_PAD.encode(json.to_string()))).unwrap()
    }

    async fn session(&mut self) -> Session {
        tokio::time::timeout(WAIT, self.sessions.recv()).await.expect("node never connected").unwrap()
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        self.server.await.unwrap();
    }
}

fn panel_cert() -> CertKey {
    let ck = rcgen::generate_simple_self_signed(vec!["wirefox-panel".to_string()]).unwrap();
    CertKey { chain: vec![ck.cert.der().clone()], key: PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()).into() }
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!("wirefox-agent-{}", wirefox_common::random_token(8)))
}

#[tokio::test(flavor = "multi_thread")]
async fn managed_node_follows_the_panel() {
    let logs = LogBuffer::new(1000);
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(logs.layer())).unwrap();

    let cert = panel_cert();
    let mut panel = Panel::start("127.0.0.1:0".parse().unwrap(), &cert).await;
    let dir = temp_dir();
    let node = Managed::new(panel.key(&panel.secret), StateDir::open(&dir).unwrap(), logs).unwrap();
    let node = tokio::spawn(node.run());

    let mut session = panel.session().await;
    let hello = session.hello().await;
    assert_eq!(hello.version, wirefox_common::VERSION);
    assert_eq!(hello.os, std::env::consts::OS);
    assert_eq!(hello.config_version, 0);
    assert!(!hello.interfaces.is_empty());

    session.send(panel_message::Msg::Welcome(Welcome { node_id: "n1".into(), status_interval_secs: 1 })).await;
    // Nothing listens on port 1, so the tunnel keeps failing; that is what the node should report.
    let config = TunnelConfig {
        version: 1,
        enabled: true,
        server: "127.0.0.1:1".into(),
        server_fingerprint: "ab".repeat(32),
        node_id: "n1".into(),
        token: "node-token".into(),
        pool_size: 2,
        max_extra: 8,
        connect_timeout_secs: 2,
        egress: Some(Egress::default()),
        ..Default::default()
    };
    session.send(panel_message::Msg::Config(config.clone())).await;
    let status = loop {
        if let node_message::Msg::Status(status) = session.recv().await {
            if status.config_version == 1 && status.tunnel_state() == TunnelState::Error {
                break status;
            }
        }
    };
    assert!(!status.last_error.is_empty());
    assert_eq!(status.connected_secs, 0);
    assert_eq!(StateDir::open(&dir).unwrap().load_tunnel().unwrap(), Some(config));

    let fetch_logs = command::Kind::FetchLogs(FetchLogs { lines: 500 });
    session.send(panel_message::Msg::Command(Command { id: "c1".into(), kind: Some(fetch_logs) })).await;
    let result = session.result().await;
    assert_eq!(result.id, "c1");
    assert!(result.error.is_empty());
    let Some(command_result::Payload::Logs(batch)) = result.payload else { panic!("no logs in {result:?}") };
    assert!(batch.lines.iter().any(|l| l.level == "INFO" && l.message.contains("connected to the panel")));

    let list = command::Kind::ListInterfaces(ListInterfaces {});
    session.send(panel_message::Msg::Command(Command { id: "c2".into(), kind: Some(list) })).await;
    let result = session.result().await;
    assert_eq!(result.id, "c2");
    let Some(command_result::Payload::Interfaces(list)) = result.payload else { panic!("no interfaces in {result:?}") };
    assert!(!list.interfaces.is_empty());

    // Take the panel down and bring it back on the same address.
    let addr = panel.addr;
    drop(session);
    panel.stop().await;
    let mut panel = Panel::start(addr, &cert).await;
    let mut session = panel.session().await;
    assert_eq!(session.hello().await.config_version, 1);

    node.abort();
    let _ = node.await;
    drop(session);
    panel.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_secret_is_rejected() {
    let mut panel = Panel::start("127.0.0.1:0".parse().unwrap(), &panel_cert()).await;
    let dir = temp_dir();
    let key = panel.key(&"00".repeat(32));
    let node = tokio::spawn(Managed::new(key, StateDir::open(&dir).unwrap(), LogBuffer::new(10)).unwrap().run());

    tokio::time::timeout(WAIT, panel.rejected.recv()).await.expect("node never tried to connect").unwrap();
    assert!(panel.sessions.try_recv().is_err());

    node.abort();
    let _ = node.await;
    panel.stop().await;
    std::fs::remove_dir_all(dir).unwrap();
}
