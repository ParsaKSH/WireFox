use std::path::Path;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use wirefox_common::proto::{EgressSpec, DEFAULT_SERVER_NAME};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    /// Server tunnel address, `host:port`.
    pub server: String,
    /// TLS SNI. Has no security role; the certificate is pinned.
    #[serde(default = "default_server_name")]
    pub server_name: String,
    /// SHA-256 fingerprint of the server certificate.
    pub server_fingerprint: String,
    pub node_id: String,
    pub token: String,
    /// Idle data connections kept open to the server, ready for new streams.
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,
    /// Cap on extra connections opened on demand during bursts.
    #[serde(default = "default_max_extra")]
    pub max_extra: u32,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// Allow proxied connections to this machine itself (loopback, its own
    /// addresses) and to link-local addresses such as cloud metadata services.
    #[serde(default)]
    pub allow_local: bool,
    /// Interface / source address for proxied traffic.
    #[serde(default)]
    pub egress: EgressSpec,
    /// Interface / source address used to reach the server.
    #[serde(default)]
    pub uplink: EgressSpec,
}

fn default_server_name() -> String {
    DEFAULT_SERVER_NAME.into()
}

fn default_pool_size() -> u32 {
    16
}

fn default_max_extra() -> u32 {
    1024
}

fn default_connect_timeout() -> u64 {
    10
}

impl NodeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let cfg: NodeConfig = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!((1..=4096).contains(&self.pool_size), "pool_size must be between 1 and 4096");
        ensure!(self.max_extra <= 65536, "max_extra must be at most 65536");
        ensure!(self.connect_timeout_secs >= 1, "connect_timeout_secs must be at least 1");
        wirefox_common::tls::parse_fingerprint(&self.server_fingerprint).context("server_fingerprint")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        server = "relay.example.com:7443"
        server_fingerprint = "0000000000000000000000000000000000000000000000000000000000000000"
        node_id = "n1"
        token = "t"
    "#;

    #[test]
    fn defaults() {
        let cfg = NodeConfig::parse(MINIMAL).unwrap();
        assert_eq!(cfg.pool_size, 16);
        assert_eq!(cfg.server_name, DEFAULT_SERVER_NAME);
        assert!(!cfg.allow_local);
        assert!(cfg.egress.is_default());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(NodeConfig::parse(&format!("{MINIMAL}\npool_size = 0")).is_err());
        assert!(NodeConfig::parse(&format!("{MINIMAL}\nmax_extra = 4294967295")).is_err());
        assert!(NodeConfig::parse(&format!("{MINIMAL}\ntypo = 1")).is_err());
        assert!(NodeConfig::parse(&MINIMAL.replace("0000", "zz")).is_err());
    }

    #[test]
    fn example_config_parses() {
        let example =
            include_str!("../../../examples/node.toml").replace("<SHA-256 printed by wirefox-server>", &"0".repeat(64));
        let cfg = NodeConfig::parse(&example).unwrap();
        assert_eq!(cfg.node_id, "node-1");
    }

    #[test]
    fn egress_section() {
        let cfg =
            NodeConfig::parse(&format!("{MINIMAL}\n[egress]\ninterface = \"eth1\"\nsource_ip = \"10.0.0.2\"")).unwrap();
        assert_eq!(cfg.egress.interface.as_deref(), Some("eth1"));
        assert_eq!(cfg.egress.source_ip, Some("10.0.0.2".parse().unwrap()));
    }
}
