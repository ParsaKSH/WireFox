use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Holds `state.json` and, by default, the TLS certificate and key.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub tls: TlsSection,
    pub tunnel: TunnelSection,
    pub socks: SocksSection,
    pub api: ApiSection,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsSection {
    /// PEM certificate and key. Default: `server.crt` / `server.key` in the data
    /// directory, generated (self-signed) on first start.
    pub cert: Option<PathBuf>,
    pub key: Option<PathBuf>,
    /// Subject alternative names for a generated certificate.
    #[serde(default)]
    pub sans: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TunnelSection {
    /// Where nodes connect.
    pub listen: SocketAddr,
    /// `host:port` nodes should use to reach the tunnel. Only used to fill in
    /// the node configs the API hands out.
    pub public_addr: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocksSection {
    /// Plain SOCKS5 listeners.
    #[serde(default)]
    pub listen: Vec<SocketAddr>,
    /// SOCKS5-over-TLS listeners.
    #[serde(default)]
    pub tls_listen: Vec<SocketAddr>,
    #[serde(default = "default_handshake_timeout")]
    pub handshake_timeout_secs: u64,
    /// How long a request may wait for a tunnel connection plus the node's dial.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSection {
    pub listen: SocketAddr,
    /// Serve the API over HTTPS with the server certificate.
    #[serde(default)]
    pub tls: bool,
    pub admin_username: String,
    /// Argon2 PHC string, from `wirefox-server hash-password`.
    pub admin_password_hash: String,
    /// SHA-256 (hex) of static API keys, from `wirefox-server gen-api-key`.
    #[serde(default)]
    pub api_key_hashes: Vec<String>,
    #[serde(default = "default_session_ttl")]
    pub session_ttl_secs: u64,
    /// Browser origins allowed to call the API (for a web panel on another origin).
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("data")
}

fn default_handshake_timeout() -> u64 {
    10
}

fn default_connect_timeout() -> u64 {
    15
}

fn default_session_ttl() -> u64 {
    12 * 3600
}

impl ServerConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut cfg: ServerConfig = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        // Relative paths are relative to the config file, not the working directory.
        let base = path.parent().unwrap_or(Path::new("."));
        cfg.data_dir = base.join(&cfg.data_dir);
        cfg.tls.cert = cfg.tls.cert.map(|p| base.join(p));
        cfg.tls.key = cfg.tls.key.map(|p| base.join(p));
        anyhow::ensure!(cfg.socks.connect_timeout_secs >= 1, "socks.connect_timeout_secs must be at least 1");
        anyhow::ensure!(cfg.socks.handshake_timeout_secs >= 1, "socks.handshake_timeout_secs must be at least 1");
        anyhow::ensure!(cfg.api.session_ttl_secs >= 60, "api.session_ttl_secs must be at least 60");
        Ok(cfg)
    }

    pub fn cert_path(&self) -> PathBuf {
        self.tls.cert.clone().unwrap_or_else(|| self.data_dir.join("server.crt"))
    }

    pub fn key_path(&self) -> PathBuf {
        self.tls.key.clone().unwrap_or_else(|| self.data_dir.join("server.key"))
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join("state.json")
    }
}
