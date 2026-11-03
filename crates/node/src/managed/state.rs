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
    /// Create the directory if needed, private to the service, and make sure
    /// it is writable.
    pub fn open(path: &Path) -> Result<Self> {
        let not_writable = || {
            format!(
                "the state directory {} is not writable; run as a user that may write it or pick another one with \
                 --state-dir",
                path.display()
            )
        };
        create_private_dir(path).with_context(not_writable)?;
        // Windows has no mode bits, and %ProgramData% lets every user read.
        #[cfg(windows)]
        super::acl::restrict_dir(path)
            .with_context(|| format!("making the state directory {} private", path.display()))?;
        let probe = path.join(".write-test");
        write_atomic_private(&probe, b"").and_then(|()| std::fs::remove_file(&probe)).with_context(not_writable)?;
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

/// A path under the system temp dir, removed again on drop.
#[cfg(test)]
pub(crate) struct TempDir(PathBuf);

#[cfg(test)]
impl TempDir {
    pub fn new(name: &str) -> Self {
        TempDir(std::env::temp_dir().join(format!("wirefox-{name}-{}", wirefox_common::random_token(8))))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pb::Egress;

    #[test]
    fn tunnel_config_round_trip() {
        let dir = TempDir::new("state");
        let state = StateDir::open(&dir.path().join("nested")).unwrap();
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
    }

    #[test]
    fn corrupt_state_is_an_error() {
        let dir = TempDir::new("state");
        let state = StateDir::open(dir.path()).unwrap();
        std::fs::write(dir.path().join(TUNNEL_FILE), "{").unwrap();
        assert!(state.load_tunnel().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_dir_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("state");
        std::fs::create_dir(dir.path()).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let Err(err) = StateDir::open(dir.path()) else {
            eprintln!("skipped: this user may write to read-only directories (root?)");
            return;
        };
        assert!(format!("{err:#}").contains("is not writable"), "{err:#}");
    }
}
