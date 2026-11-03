//! Runs the tunnel described by the config the panel pushed, replacing it
//! whenever a new config arrives.

use std::net::IpAddr;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use wirefox_common::proto::{EgressSpec, DEFAULT_SERVER_NAME};

use crate::config::NodeConfig;
use crate::managed::state::StateDir;
use crate::pb::{Egress, TunnelConfig};
use crate::tunnel::{Monitor, Node};

pub struct Supervisor {
    state: StateDir,
    monitor: Arc<Monitor>,
    applied: Option<TunnelConfig>,
    running: Option<JoinHandle<()>>,
    /// Why the last config from the panel was not applied.
    config_error: Option<String>,
}

impl Supervisor {
    pub fn new(state: StateDir) -> Self {
        Supervisor { state, monitor: Arc::default(), applied: None, running: None, config_error: None }
    }

    pub fn state(&self) -> &StateDir {
        &self.state
    }

    pub fn monitor(&self) -> &Arc<Monitor> {
        &self.monitor
    }

    /// Version of the applied config, 0 if none.
    pub fn config_version(&self) -> u64 {
        self.applied.as_ref().map_or(0, |c| c.version)
    }

    pub fn config_error(&self) -> Option<&str> {
        self.config_error.as_deref()
    }

    /// The interface used to reach the WireFox server, which the panel
    /// connection uses as well. Falls back to the default route while there's
    /// no usable config.
    pub fn uplink(&self) -> EgressSpec {
        self.applied.as_ref().and_then(|c| egress_spec(c.uplink.as_ref()).ok()).unwrap_or_default()
    }

    /// Switch to `cfg`. A config that can't be used is reported through
    /// [`config_error`](Self::config_error) and the current tunnel keeps running.
    pub async fn apply(&mut self, cfg: TunnelConfig) {
        let unchanged = self.applied.as_ref().is_some_and(|c| c.version == cfg.version);
        if unchanged && (self.running.is_some() || !cfg.enabled) {
            return;
        }
        let version = cfg.version;
        self.config_error = match self.replace(cfg).await {
            Ok(()) => None,
            Err(e) => {
                warn!(version, "tunnel config not applied: {e:#}");
                Some(format!("tunnel config {version}: {e:#}"))
            }
        };
    }

    /// Restart the tunnel with the applied config.
    pub async fn restart(&mut self) -> Result<()> {
        let Some(cfg) = &self.applied else { bail!("the node has no tunnel config yet") };
        if !cfg.enabled {
            bail!("the tunnel is disabled");
        }
        let version = cfg.version;
        let node = Node::with_monitor(node_config(cfg)?, self.monitor.clone())?;
        self.stop().await;
        info!(version, "restarting the tunnel");
        self.running = Some(tokio::spawn(node.run()));
        Ok(())
    }

    async fn replace(&mut self, cfg: TunnelConfig) -> Result<()> {
        // Build the new node first, so a bad config leaves the old tunnel alone.
        let node = if cfg.enabled { Some(Node::with_monitor(node_config(&cfg)?, self.monitor.clone())?) } else { None };
        self.stop().await;
        match node {
            Some(node) => {
                info!(version = cfg.version, server = %cfg.server, "starting the tunnel");
                self.running = Some(tokio::spawn(node.run()));
            }
            None => info!(version = cfg.version, "the tunnel is disabled"),
        }
        let saved = self.state.save_tunnel(&cfg);
        self.applied = Some(cfg);
        saved
    }

    /// Streams the old node is relaying run in their own tasks and survive this.
    async fn stop(&mut self) {
        if let Some(task) = self.running.take() {
            task.abort();
            // Wait until it is gone, so it can't report state over the next node.
            let _ = task.await;
            self.monitor.set_stopped();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(task) = &self.running {
            task.abort();
        }
    }
}

/// Translate a config pushed by the panel, validating it like a config file.
pub fn node_config(cfg: &TunnelConfig) -> Result<NodeConfig> {
    let node = NodeConfig {
        server: cfg.server.clone(),
        server_name: if cfg.server_name.is_empty() { DEFAULT_SERVER_NAME.into() } else { cfg.server_name.clone() },
        server_fingerprint: cfg.server_fingerprint.clone(),
        node_id: cfg.node_id.clone(),
        token: cfg.token.clone(),
        pool_size: cfg.pool_size,
        max_extra: cfg.max_extra,
        connect_timeout_secs: cfg.connect_timeout_secs.into(),
        allow_local: cfg.allow_local,
        egress: egress_spec(cfg.egress.as_ref()).context("egress")?,
        uplink: egress_spec(cfg.uplink.as_ref()).context("uplink")?,
    };
    for (name, value) in [("server", &node.server), ("node_id", &node.node_id), ("token", &node.token)] {
        if value.is_empty() {
            bail!("{name} is missing");
        }
    }
    node.validate()?;
    Ok(node)
}

fn egress_spec(egress: Option<&Egress>) -> Result<EgressSpec> {
    let Some(egress) = egress else { return Ok(EgressSpec::default()) };
    let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let source_ip = non_empty(&egress.source_ip)
        .map(|ip| ip.parse::<IpAddr>().with_context(|| format!("invalid source_ip {ip:?}")))
        .transpose()?;
    Ok(EgressSpec { interface: non_empty(&egress.interface), source_ip })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TunnelConfig {
        TunnelConfig {
            version: 3,
            enabled: true,
            server: "relay.example.com:7443".into(),
            server_fingerprint: "ab".repeat(32),
            node_id: "n1".into(),
            token: "t".into(),
            pool_size: 8,
            max_extra: 100,
            connect_timeout_secs: 5,
            egress: Some(Egress { interface: "eth1".into(), source_ip: "10.0.0.2".into() }),
            uplink: Some(Egress { interface: String::new(), source_ip: String::new() }),
            ..Default::default()
        }
    }

    #[test]
    fn converts_to_node_config() {
        let node = node_config(&config()).unwrap();
        assert_eq!(node.server, "relay.example.com:7443");
        assert_eq!(node.server_name, DEFAULT_SERVER_NAME);
        assert_eq!((node.pool_size, node.max_extra, node.connect_timeout_secs), (8, 100, 5));
        assert_eq!(node.egress.interface.as_deref(), Some("eth1"));
        assert_eq!(node.egress.source_ip, Some("10.0.0.2".parse().unwrap()));
        assert!(node.uplink.is_default());

        let named = node_config(&TunnelConfig { server_name: "relay".into(), egress: None, ..config() }).unwrap();
        assert_eq!(named.server_name, "relay");
        assert!(named.egress.is_default());
    }

    #[test]
    fn rejects_bad_ips() {
        let bad =
            TunnelConfig { egress: Some(Egress { interface: String::new(), source_ip: "10.0.0".into() }), ..config() };
        let err = node_config(&bad).unwrap_err();
        assert!(format!("{err:#}").contains("egress: invalid source_ip"), "{err:#}");
        let bad = TunnelConfig {
            uplink: Some(Egress { interface: String::new(), source_ip: "fe80::zz".into() }),
            ..config()
        };
        assert!(format!("{:#}", node_config(&bad).unwrap_err()).contains("uplink"));
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(node_config(&TunnelConfig { pool_size: 0, ..config() }).is_err());
        assert!(node_config(&TunnelConfig { connect_timeout_secs: 0, ..config() }).is_err());
        assert!(node_config(&TunnelConfig { server_fingerprint: "abcd".into(), ..config() }).is_err());
        assert!(node_config(&TunnelConfig { token: String::new(), ..config() }).is_err());
        assert!(node_config(&TunnelConfig { server: String::new(), ..config() }).is_err());
    }

    #[tokio::test]
    async fn applies_configs_by_version() {
        let dir = std::env::temp_dir().join(format!("wirefox-supervisor-{}", wirefox_common::random_token(8)));
        let mut sup = Supervisor::new(StateDir::open(&dir).unwrap());
        // Nothing listens on port 1, so the tunnel just keeps retrying.
        let cfg = TunnelConfig { server: "127.0.0.1:1".into(), ..config() };
        sup.apply(cfg.clone()).await;
        assert_eq!(sup.config_version(), 3);
        assert!(sup.config_error().is_none());
        assert!(sup.running.is_some());
        assert_eq!(sup.state().load_tunnel().unwrap(), Some(cfg.clone()));

        // A broken config is reported and the running one is kept.
        sup.apply(TunnelConfig { version: 4, pool_size: 0, ..cfg.clone() }).await;
        assert!(sup.config_error().unwrap().contains("pool_size"));
        assert_eq!(sup.config_version(), 3);
        assert!(sup.running.is_some());

        sup.apply(TunnelConfig { version: 5, enabled: false, ..cfg.clone() }).await;
        assert!(sup.config_error().is_none());
        assert!(sup.running.is_none());
        assert_eq!(sup.monitor().status().state, crate::tunnel::TunnelState::Stopped);
        assert!(sup.restart().await.is_err());
        assert_eq!(sup.state().load_tunnel().unwrap().unwrap().version, 5);

        drop(sup);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
