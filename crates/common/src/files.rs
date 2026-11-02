//! Writing files that hold secrets.
//!
//! On Unix they are created with mode 0600. On Windows they inherit the ACL of
//! the directory they are written to, so keep that directory private.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Write `data` to `path`, readable only by the owner, and flush it to disk.
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    // `mode` only applies to new files; tighten an existing one too.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(data)?;
    f.sync_all()
}

/// Replace `path` atomically: write a temp file next to it, then rename it over.
/// After a crash the file holds either the old or the new contents, never a mix.
pub fn write_atomic_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.file_name().unwrap_or_default().to_owned();
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);
    write_private(&tmp, data)?;
    fs::rename(&tmp, path)?;
    // Persist the rename itself. (Windows has no equivalent for directories.)
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_contents() {
        let dir = std::env::temp_dir().join(format!("wirefox-files-{}", crate::random_token(8)));
        let path = dir.join("state.json");
        write_atomic_private(&path, b"one").unwrap();
        write_atomic_private(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert!(!dir.join("state.json.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
