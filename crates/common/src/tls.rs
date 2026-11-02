//! TLS setup. The server uses a (normally self-signed) certificate and nodes pin
//! its SHA-256 fingerprint, so there is no CA involved and no CA can MITM the tunnel.

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{ring, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{CipherSuite, ClientConfig, DigitallySignedStruct, ServerConfig, SignatureScheme};

use crate::files::write_private;

pub struct CertKey {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

impl CertKey {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.chain[0])
    }
}

/// Load the PEM certificate and key, generating a self-signed pair if neither exists yet.
pub fn load_or_generate(cert_path: &Path, key_path: &Path, sans: &[String]) -> Result<CertKey> {
    match (cert_path.exists(), key_path.exists()) {
        (true, true) => load(cert_path, key_path),
        (false, false) => {
            generate(cert_path, key_path, sans)?;
            load(cert_path, key_path)
        }
        _ => bail!("only one of {} and {} exists; provide both or neither", cert_path.display(), key_path.display()),
    }
}

pub fn load(cert_path: &Path, key_path: &Path) -> Result<CertKey> {
    let chain = CertificateDer::pem_file_iter(cert_path)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .with_context(|| format!("reading certificate from {}", cert_path.display()))?;
    if chain.is_empty() {
        bail!("no certificate found in {}", cert_path.display());
    }
    let key = PrivateKeyDer::from_pem_file(key_path)
        .with_context(|| format!("reading private key from {}", key_path.display()))?;
    Ok(CertKey { chain, key })
}

pub fn generate(cert_path: &Path, key_path: &Path, sans: &[String]) -> Result<()> {
    let sans = if sans.is_empty() { vec![crate::proto::DEFAULT_SERVER_NAME.to_string()] } else { sans.to_vec() };
    let ck = rcgen::generate_simple_self_signed(sans).context("generating self-signed certificate")?;
    for p in [cert_path, key_path] {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
    }
    std::fs::write(cert_path, ck.cert.pem())?;
    write_private(key_path, ck.signing_key.serialize_pem().as_bytes())?;
    Ok(())
}

/// Lowercase hex SHA-256 of a DER certificate.
pub fn fingerprint(cert: &CertificateDer<'_>) -> String {
    crate::sha256_hex(cert.as_ref())
}

/// Parse a SHA-256 fingerprint. Colons, dashes, spaces and upper case are accepted.
pub fn parse_fingerprint(s: &str) -> Result<[u8; 32]> {
    let cleaned: String = s.chars().filter(|c| !matches!(c, ':' | ' ' | '-')).collect();
    let bytes = hex::decode(cleaned.to_ascii_lowercase()).context("fingerprint is not hex")?;
    bytes.try_into().map_err(|_| anyhow::anyhow!("fingerprint must be 32 bytes (SHA-256)"))
}

/// Server-side config. The tunnel listener is TLS 1.3 only; SOCKS-over-TLS and
/// the API also accept TLS 1.2 for older clients. The server follows the
/// client's cipher preference, so each node gets the suite that is fastest on
/// its own CPU.
pub fn server_config(ck: &CertKey, tls13_only: bool) -> Result<Arc<ServerConfig>> {
    let versions: &[&rustls::SupportedProtocolVersion] =
        if tls13_only { &[&rustls::version::TLS13] } else { &[&rustls::version::TLS13, &rustls::version::TLS12] };
    let mut cfg = ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_protocol_versions(versions)?
        .with_no_client_auth()
        .with_single_cert(ck.chain.clone(), ck.key.clone_key())?;
    // Session tickets make the node's frequent data-connection handshakes cheap.
    cfg.ticketer = ring::Ticketer::new()?;
    cfg.session_storage = rustls::server::ServerSessionMemoryCache::new(4096);
    Ok(Arc::new(cfg))
}

/// Client config that trusts exactly one server certificate, identified by its SHA-256.
pub fn pinned_client_config(fingerprint_hex: &str) -> Result<Arc<ClientConfig>> {
    let provider = Arc::new(client_provider());
    let verifier = PinnedVerifier {
        fingerprint: parse_fingerprint(fingerprint_hex)?,
        algs: provider.signature_verification_algorithms,
    };
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    cfg.resumption = rustls::client::Resumption::in_memory_sessions(1024);
    Ok(Arc::new(cfg))
}

/// ring's default order is AES-256, AES-128, ChaCha20. Put AES-128-GCM first when
/// the CPU has AES and carry-less multiply instructions (same security margin for
/// our purposes, fewer rounds), and ChaCha20 first when it doesn't: on CPUs like
/// the Cortex-A7, software AES is several times slower than ChaCha20.
fn client_provider() -> CryptoProvider {
    let preferred = if has_hardware_aes() {
        CipherSuite::TLS13_AES_128_GCM_SHA256
    } else {
        CipherSuite::TLS13_CHACHA20_POLY1305_SHA256
    };
    let mut provider = ring::default_provider();
    provider.cipher_suites.sort_by_key(|s| s.suite() != preferred);
    provider
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn has_hardware_aes() -> bool {
    std::arch::is_x86_feature_detected!("aes") && std::arch::is_x86_feature_detected!("pclmulqdq")
}

#[cfg(target_arch = "aarch64")]
fn has_hardware_aes() -> bool {
    std::arch::is_aarch64_feature_detected!("aes") && std::arch::is_aarch64_feature_detected!("pmull")
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64", target_arch = "aarch64")))]
fn has_hardware_aes() -> bool {
    false
}

#[derive(Debug)]
struct PinnedVerifier {
    fingerprint: [u8; 32],
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if crate::ct_eq(&crate::sha256(end_entity.as_ref()), &self.fingerprint) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("server certificate fingerprint mismatch".into()))
        }
    }

    // Required by the trait, but never called: the client only speaks TLS 1.3.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    // Checking the handshake signature is what proves the server holds the pinned cert's key.
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_formats() {
        let hex = "ab".repeat(32);
        let expected = [0xab; 32];
        assert_eq!(parse_fingerprint(&hex).unwrap(), expected);
        assert_eq!(parse_fingerprint(&hex.to_uppercase()).unwrap(), expected);
        let colons = vec!["AB"; 32].join(":");
        assert_eq!(parse_fingerprint(&colons).unwrap(), expected);
        assert!(parse_fingerprint("abcd").is_err());
        assert!(parse_fingerprint(&"zz".repeat(32)).is_err());
    }

    #[test]
    fn generate_and_load() {
        let dir = std::env::temp_dir().join(format!("wirefox-tls-{}", crate::random_token(8)));
        let (cert, key) = (dir.join("server.crt"), dir.join("server.key"));
        let first = load_or_generate(&cert, &key, &[]).unwrap();
        let again = load_or_generate(&cert, &key, &[]).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert!(pinned_client_config(&first.fingerprint()).is_ok());
        assert!(server_config(&first, true).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preferred_suite_comes_first() {
        let first = client_provider().cipher_suites[0].suite();
        assert!(matches!(first, CipherSuite::TLS13_AES_128_GCM_SHA256 | CipherSuite::TLS13_CHACHA20_POLY1305_SHA256));
    }
}
