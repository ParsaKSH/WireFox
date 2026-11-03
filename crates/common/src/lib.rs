//! Code shared by the WireFox server and exit-node: the wire protocol, TLS setup
//! and a few socket helpers.

pub mod files;
pub mod net;
pub mod proto;
pub mod tls;

use sha2::{Digest, Sha256};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Set up logging. `RUST_LOG` overrides `default_level`.
pub fn init_tracing(default_level: &str) {
    use std::io::IsTerminal;
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    // Colours only on a terminal; under systemd they'd end up as junk in the journal.
    let ansi = std::io::stdout().is_terminal();
    tracing_subscriber::fmt().with_env_filter(filter).with_target(false).with_ansi(ansi).init();
}

/// Random hex token made of `bytes` bytes from the OS CSPRNG.
pub fn random_token(bytes: usize) -> String {
    use rand::Rng;
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(sha256(data))
}

/// Constant-time comparison for secrets.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.ct_eq(b))
}
