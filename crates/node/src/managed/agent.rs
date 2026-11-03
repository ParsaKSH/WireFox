//! The node's side of the agent protocol: one long-lived gRPC stream to the
//! panel, re-established with backoff whenever it drops.

use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use hyper_util::rt::TokioIo;
use tokio::sync::mpsc;
use tokio::time::{Interval, MissedTickBehavior};
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::TlsConnector;
use tokio_stream::wrappers::ReceiverStream;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic::{Code, Request};
use tracing::{error, info, warn};
use wirefox_common::net::tune_tcp;
use wirefox_common::proto::EgressSpec;

use crate::egress::{list_interfaces, Dialer};
use crate::managed::logs::LogBuffer;
use crate::managed::state::StateDir;
use crate::managed::supervisor::Supervisor;
use crate::managed::NodeKey;
use crate::pb::agent_client::AgentClient;
use crate::pb::{
    command, command_result, node_message, panel_message, Command, CommandResult, Hello, Interface, InterfaceList,
    LogBatch, NodeMessage, Status, TunnelState,
};
use crate::tunnel::{self, hostname, parse_host_port};

/// TLS SNI the panel's certificate is issued for. The certificate is pinned,
/// so this has no security role.
const PANEL_SERVER_NAME: &str = "wirefox-panel";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// Backoff after the panel refused our key. It stays retrying, because
/// restoring a panel backup can make the key valid again.
const REJECTED_BACKOFF: Duration = Duration::from_secs(60);
/// A connection that lasted this long resets the backoff.
const STABLE_CONNECTION: Duration = Duration::from_secs(60);

pub struct Managed {
    key: NodeKey,
    tls: Arc<ClientConfig>,
    supervisor: Supervisor,
    logs: LogBuffer,
    started: Instant,
}

impl Managed {
    pub fn new(key: NodeKey, state: StateDir, logs: LogBuffer) -> Result<Self> {
        let mut tls = Arc::unwrap_or_clone(wirefox_common::tls::pinned_client_config(&key.fingerprint)?);
        // gRPC servers insist on negotiating HTTP/2 through ALPN.
        tls.alpn_protocols = vec![b"h2".to_vec()];
        Ok(Managed { key, tls: Arc::new(tls), supervisor: Supervisor::new(state), logs, started: Instant::now() })
    }

    /// Bring up the saved tunnel, then stay connected to the panel forever.
    pub async fn run(mut self) {
        match self.supervisor.state().load_tunnel() {
            Ok(Some(cfg)) => self.supervisor.apply(cfg).await,
            Ok(None) => info!("no tunnel config yet, waiting for the panel to send one"),
            Err(e) => warn!("ignoring the saved tunnel config: {e:#}"),
        }

        let mut backoff = MIN_BACKOFF;
        let mut default_route = false;
        loop {
            let uplink = self.supervisor.uplink();
            let route = if default_route { EgressSpec::default() } else { uplink.clone() };
            let started = Instant::now();
            let result = self.session(route).await;
            if started.elapsed() > STABLE_CONNECTION {
                backoff = MIN_BACKOFF;
            }
            // The uplink comes from the panel and may be wrong (a typo, a
            // modem that went away). Every other try after a failure goes over
            // the default route, so the panel can still reach the node to fix it.
            default_route = result.is_err() && !default_route && !uplink.is_default();
            let delay = match result {
                Ok(()) => {
                    info!("the panel closed the connection");
                    backoff
                }
                Err(e) if e.downcast_ref::<tonic::Status>().is_some_and(|s| s.code() == Code::Unauthenticated) => {
                    error!(
                        "the panel rejected this node's key: it was revoked or replaced, or is not a key of this \
                         panel; run the node with the key the panel shows for it (retrying in {} s)",
                        REJECTED_BACKOFF.as_secs()
                    );
                    REJECTED_BACKOFF
                }
                Err(e) => {
                    warn!(panel = %self.key.panel, "panel connection failed: {e:#}");
                    backoff
                }
            };
            if default_route {
                info!(%uplink, "trying the panel over the default route next");
            }
            let jitter = Duration::from_millis(rand::random::<u64>() % (delay.as_millis() as u64 / 4 + 1));
            tokio::time::sleep(delay + jitter).await;
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    }

    async fn session(&mut self, route: EgressSpec) -> Result<()> {
        let mut client = AgentClient::new(self.connect(route).await?);
        let (tx, rx) = mpsc::channel(32);
        let outbound = Outbound(tx);
        outbound.send(node_message::Msg::Hello(self.hello())).await?;
        let mut request = Request::new(ReceiverStream::new(rx));
        let bearer = MetadataValue::try_from(format!("Bearer {}", self.key.secret)).context("invalid node secret")?;
        request.metadata_mut().insert("authorization", bearer);
        let mut inbound = client.connect(request).await?.into_inner();
        info!(panel = %self.key.panel, "connected to the panel");

        let mut tunnel_changes = self.supervisor.monitor().subscribe();
        // Status reports start once the panel has said how often it wants them.
        let mut status_timer: Option<Interval> = None;
        loop {
            tokio::select! {
                msg = inbound.message() => {
                    let Some(msg) = msg? else { return Ok(()) };
                    match msg.msg {
                        Some(panel_message::Msg::Welcome(welcome)) => {
                            info!(node_id = %welcome.node_id, "registered with the panel");
                            let every = Duration::from_secs(welcome.status_interval_secs.max(1).into());
                            let mut timer = tokio::time::interval(every);
                            timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
                            status_timer = Some(timer);
                        }
                        Some(panel_message::Msg::Config(cfg)) => {
                            self.supervisor.apply(cfg).await;
                            outbound.send(node_message::Msg::Status(self.status())).await?;
                        }
                        Some(panel_message::Msg::Command(cmd)) => {
                            let result = self.execute(cmd).await;
                            outbound.send(node_message::Msg::Result(result)).await?;
                        }
                        // A message type added after this node was built.
                        None => {}
                    }
                }
                () = tick(&mut status_timer) => outbound.send(node_message::Msg::Status(self.status())).await?,
                Ok(()) = tunnel_changes.changed(), if status_timer.is_some() => {
                    outbound.send(node_message::Msg::Status(self.status())).await?;
                }
            }
        }
    }

    /// Open the HTTP/2 connection: TCP through `route`, then TLS with the
    /// panel's certificate pinned.
    async fn connect(&self, route: EgressSpec) -> Result<Channel> {
        let panel = Arc::new(parse_host_port(&self.key.panel)?);
        let dialer = Arc::new(Dialer::new(route, CONNECT_TIMEOUT, true));
        let tls = TlsConnector::from(self.tls.clone());
        let connector = tower::service_fn(move |_: Uri| {
            let (panel, dialer, tls) = (panel.clone(), dialer.clone(), tls.clone());
            async move {
                let tcp = dialer.dial(&panel).await?;
                tune_tcp(&tcp)?;
                let server_name = ServerName::try_from(PANEL_SERVER_NAME).map_err(io::Error::other)?;
                let stream = tls.connect(server_name, tcp).await?;
                Ok::<_, io::Error>(TokioIo::new(stream))
            }
        });
        // TLS is handled by the connector, so to tonic this is plain HTTP/2.
        let channel = Endpoint::from_shared(format!("http://{}", self.key.panel))?
            .connect_timeout(CONNECT_TIMEOUT)
            .http2_keep_alive_interval(KEEPALIVE_INTERVAL)
            .keep_alive_timeout(KEEPALIVE_TIMEOUT)
            .keep_alive_while_idle(true)
            .connect_with_connector(connector)
            .await
            .map_err(|e| {
                // Each layer of tonic's error repeats its cause; the innermost one says it all.
                let cause = std::iter::successors(Some(&e as &dyn std::error::Error), |e| e.source()).last();
                anyhow!("connecting to the panel: {}", cause.unwrap_or(&e))
            })?;
        Ok(channel)
    }

    fn hello(&self) -> Hello {
        Hello {
            version: wirefox_common::VERSION.into(),
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            hostname: hostname(),
            interfaces: interfaces(),
            config_version: self.supervisor.config_version(),
        }
    }

    fn status(&self) -> Status {
        let monitor = self.supervisor.monitor();
        let tunnel = monitor.status();
        let tunnel_state = match tunnel.state {
            tunnel::TunnelState::Stopped => TunnelState::Stopped,
            tunnel::TunnelState::Connecting => TunnelState::Connecting,
            tunnel::TunnelState::Connected => TunnelState::Connected,
            tunnel::TunnelState::Error => TunnelState::Error,
        };
        // A rejected config matters more than why the running tunnel is down,
        // and that more than an egress that fails every stream of a working tunnel.
        let last_error = self
            .supervisor
            .config_error()
            .map(str::to_string)
            .or(tunnel.last_error)
            .or_else(|| self.supervisor.egress_error());
        Status {
            config_version: self.supervisor.config_version(),
            tunnel_state: tunnel_state.into(),
            last_error: last_error.unwrap_or_default(),
            uptime_secs: self.started.elapsed().as_secs(),
            connected_secs: tunnel.connected_since.map_or(0, |t| t.elapsed().as_secs()),
            active_streams: monitor.active_streams(),
            total_streams: monitor.total_streams(),
        }
    }

    async fn execute(&mut self, cmd: Command) -> CommandResult {
        let mut result = CommandResult { id: cmd.id, ..Default::default() };
        match cmd.kind {
            Some(command::Kind::RestartTunnel(_)) => {
                if let Err(e) = self.supervisor.restart().await {
                    result.error = format!("{e:#}");
                }
            }
            Some(command::Kind::ListInterfaces(_)) => {
                result.payload = Some(command_result::Payload::Interfaces(InterfaceList { interfaces: interfaces() }));
            }
            Some(command::Kind::FetchLogs(fetch)) => {
                let lines = self.logs.tail(fetch.lines as usize);
                result.payload = Some(command_result::Payload::Logs(LogBatch { lines }));
            }
            None => result.error = "unknown command; this node may need an upgrade".into(),
        }
        result
    }
}

/// The sending half of the stream to the panel.
struct Outbound(mpsc::Sender<NodeMessage>);

impl Outbound {
    async fn send(&self, msg: node_message::Msg) -> Result<()> {
        self.0.send(NodeMessage { msg: Some(msg) }).await.context("the stream to the panel is closed")
    }
}

/// Wait for the next tick of `timer`, or forever if there is none.
async fn tick(timer: &mut Option<Interval>) {
    match timer {
        Some(timer) => {
            timer.tick().await;
        }
        None => std::future::pending().await,
    }
}

fn interfaces() -> Vec<Interface> {
    list_interfaces()
        .into_iter()
        .map(|i| Interface {
            name: i.name,
            adapter_name: i.adapter_name.unwrap_or_default(),
            index: i.index.unwrap_or(0),
            up: i.up,
            addrs: i.addrs.iter().map(ToString::to_string).collect(),
        })
        .collect()
}
