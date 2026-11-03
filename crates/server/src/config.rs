use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::balance::Strategy;
use crate::settings::Settings;

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
    /// Time allowed for the TLS and SOCKS handshakes, 1 to 600 seconds.
    #[serde(default = "default_handshake_timeout")]
    pub handshake_timeout_secs: u64,
    /// How long a request may wait for a tunnel connection plus the node's dial,
    /// 1 to 600 seconds.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// How connections are spread over nodes: `round-robin`, `least-ping`,
    /// `least-load` or `random`. Users can override it through the API.
    #[serde(default)]
    pub balance: Strategy,
}

impl SocksSection {
    /// The initial values of the settings the API can change at runtime.
    pub fn settings(&self) -> Settings {
        Settings {
            balance: self.balance,
            handshake_timeout_secs: self.handshake_timeout_secs,
            connect_timeout_secs: self.connect_timeout_secs,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiSection {
    pub listen: SocketAddr,
    /// Serve the API over HTTPS with the server certificate.
    #[serde(default)]
    pub tls: bool,
    /// Admin login for `/auth/login`. Set both or neither; without them only
    /// the API keys work.
    pub admin_username: Option<String>,
    /// Argon2 PHC string, from `wirefox-server hash-password`.
    pub admin_password_hash: Option<String>,
    /// SHA-256 (hex) of static API keys, from `wirefox-server gen-api-key`.
    #[serde(default)]
    pub api_key_hashes: Vec<String>,
    #[serde(default = "default_session_ttl")]
    pub session_ttl_secs: u64,
    /// Browser origins allowed to call the API (for a web panel on another origin).
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

impl ApiSection {
    /// Admin username and password hash, if admin login is enabled.
    pub fn admin(&self) -> Option<(&str, &str)> {
        Some((self.admin_username.as_deref()?, self.admin_password_hash.as_deref()?))
    }
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
        cfg.validate().with_context(|| format!("checking {}", path.display()))?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        self.socks.settings().validate().map_err(|e| anyhow::anyhow!("socks.{e}"))?;
        anyhow::ensure!(self.api.session_ttl_secs >= 60, "api.session_ttl_secs must be at least 60");
        anyhow::ensure!(
            self.api.admin_username.is_some() == self.api.admin_password_hash.is_some(),
            "api.admin_username and api.admin_password_hash must be set together"
        );
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(api: &str, socks: &str) -> Result<ServerConfig> {
        let text = format!(
            "[tunnel]\nlisten = \"127.0.0.1:7443\"\n[socks]\nlisten = []\n{socks}\n[api]\nlisten = \"127.0.0.1:9090\"\n{api}"
        );
        let cfg: ServerConfig = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    #[test]
    fn admin_login_is_optional() {
        let cfg = parse("", "").unwrap();
        assert_eq!(cfg.api.admin(), None);

        let cfg = parse("admin_username = \"admin\"\nadmin_password_hash = \"$argon2id$x\"", "").unwrap();
        assert_eq!(cfg.api.admin(), Some(("admin", "$argon2id$x")));
    }

    #[test]
    fn admin_login_needs_both_fields() {
        for api in ["admin_username = \"admin\"", "admin_password_hash = \"$argon2id$x\""] {
            let err = parse(api, "").unwrap_err().to_string();
            assert!(err.contains("set together"), "{err}");
        }
    }

    #[test]
    fn socks_settings() {
        let cfg = parse("", "").unwrap();
        assert_eq!(
            cfg.socks.settings(),
            Settings { balance: Strategy::LeastLoad, handshake_timeout_secs: 10, connect_timeout_secs: 15 }
        );
        let cfg = parse("", "balance = \"random\"\nconnect_timeout_secs = 600").unwrap();
        assert_eq!(cfg.socks.settings().balance, Strategy::Random);
        let err = parse("", "handshake_timeout_secs = 0").unwrap_err().to_string();
        assert!(err.contains("socks.handshake_timeout_secs"), "{err}");
        assert!(parse("", "connect_timeout_secs = 601").is_err());
    }
}
