//! The managed node's state directory. It holds the last tunnel config the
//! panel pushed, so the node can bring its tunnel up without the panel.

use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use wirefox_common::files::write_atomic_private;

use crate::pb::TunnelConfig;

const TUNNEL_FILE: &str = "tunnel.json";

/// `/var/lib/wirefox-node` on Unix, `%ProgramData%\wirefox-node` on Windows.
pub fn default_state_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    #[cfg(not(windows))]
    let base = PathBuf::from("/var/lib");
    base.join("wirefox-node")
}

pub struct StateDir {
    path: PathBuf,
}

impl StateDir {
    /// Create the directory if needed and make sure it is writable.
    pub fn open(path: &Path) -> Result<Self> {
        let probe = path.join(".write-test");
        create_private_dir(path)
            .and_then(|()| write_atomic_private(&probe, b""))
            .and_then(|()| std::fs::remove_file(&probe))
            .with_context(|| {
                format!(
                    "the state directory {} is not writable; run as a user that may write it or pick another one \
                     with --state-dir",
                    path.display()
                )
            })?;
        Ok(StateDir { path: path.to_owned() })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The last applied tunnel config, if there is one.
    pub fn load_tunnel(&self) -> Result<Option<TunnelConfig>> {
        let path = self.path.join(TUNNEL_FILE);
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        serde_json::from_slice(&data).map(Some).with_context(|| format!("parsing {}", path.display()))
    }

    /// The file holds the node's token, so it is private to the service user.
    pub fn save_tunnel(&self, cfg: &TunnelConfig) -> Result<()> {
        let path = self.path.join(TUNNEL_FILE);
        let json = serde_json::to_vec_pretty(cfg)?;
        write_atomic_private(&path, &json).with_context(|| format!("writing {}", path.display()))
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pb::Egress;

    #[test]
    fn tunnel_config_round_trip() {
        let dir = std::env::temp_dir().join(format!("wirefox-state-{}", wirefox_common::random_token(8)));
        let state = StateDir::open(&dir.join("nested")).unwrap();
        assert!(state.load_tunnel().unwrap().is_none());

        let cfg = TunnelConfig {
            version: 7,
            enabled: true,
            server: "relay.example.com:7443".into(),
            server_fingerprint: "ab".repeat(32),
            node_id: "n1".into(),
            token: "secret-token".into(),
            pool_size: 4,
            egress: Some(Egress { interface: "eth1".into(), source_ip: String::new() }),
            ..Default::default()
        };
        state.save_tunnel(&cfg).unwrap();
        assert_eq!(state.load_tunnel().unwrap(), Some(cfg));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(state.path().join(TUNNEL_FILE)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            let mode = std::fs::metadata(state.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_state_is_an_error() {
        let dir = std::env::temp_dir().join(format!("wirefox-state-{}", wirefox_common::random_token(8)));
        let state = StateDir::open(&dir).unwrap();
        std::fs::write(dir.join(TUNNEL_FILE), "{").unwrap();
        assert!(state.load_tunnel().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_dir_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("wirefox-state-{}", wirefox_common::random_token(8)));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        // Root may write anyway, so only check the outcome when it fails.
        if let Err(err) = StateDir::open(&dir) {
            assert!(format!("{err:#}").contains("is not writable"), "{err:#}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
