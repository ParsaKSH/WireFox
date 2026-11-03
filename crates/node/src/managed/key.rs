//! The node key: everything a node needs to find and authenticate to the panel,
//! in one string the operator can paste.

use std::fmt;

use anyhow::{bail, ensure, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;

const PREFIX: &str = "wfx1.";

#[derive(Clone, PartialEq, Eq)]
pub struct NodeKey {
    /// `host:port` of the panel's gRPC listener.
    pub panel: String,
    /// Hex SHA-256 of the panel's certificate.
    pub fingerprint: String,
    /// Sent as a Bearer token; the panel only stores its hash.
    pub secret: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    panel: String,
    fp: String,
    secret: String,
}

impl NodeKey {
    /// Parse `"wfx1." + base64url_nopad(JSON)`.
    pub fn parse(key: &str) -> Result<Self> {
        let Some(encoded) = key.trim().strip_prefix(PREFIX) else {
            bail!("not a WireFox node key (it should start with \"{PREFIX}\")");
        };
        let json = URL_SAFE_NO_PAD.decode(encoded).context("node key is not valid base64url")?;
        let payload: Payload = serde_json::from_slice(&json).context("node key holds malformed JSON")?;

        wirefox_common::tls::parse_fingerprint(&payload.fp).context("node key has an invalid panel fingerprint")?;
        ensure!(
            payload.secret.len() == 64 && payload.secret.bytes().all(|b| b.is_ascii_hexdigit()),
            "node key has an invalid secret"
        );
        let (host, port) = payload.panel.rsplit_once(':').context("node key's panel address must be host:port")?;
        ensure!(!host.is_empty() && port.parse::<u16>().is_ok(), "node key's panel address must be host:port");

        Ok(NodeKey { panel: payload.panel, fingerprint: payload.fp, secret: payload.secret })
    }
}

impl fmt::Debug for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeKey")
            .field("panel", &self.panel)
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(panel: &str, fingerprint: &str, secret: &str) -> String {
        let json = serde_json::json!({ "panel": panel, "fp": fingerprint, "secret": secret });
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json.to_string()))
    }

    fn fp() -> String {
        "ab".repeat(32)
    }

    fn secret() -> String {
        "0123456789abcdef".repeat(4)
    }

    #[test]
    fn parses_a_good_key() {
        let key = NodeKey::parse(&format!("  {}\n", encode("panel.example.com:7444", &fp(), &secret()))).unwrap();
        assert_eq!(key, NodeKey { panel: "panel.example.com:7444".into(), fingerprint: fp(), secret: secret() });
        assert!(!format!("{key:?}").contains(&secret()));
        assert!(NodeKey::parse(&encode("[2001:db8::1]:7444", &fp(), &secret())).is_ok());
    }

    #[test]
    fn rejects_wrong_prefix() {
        let key = encode("panel:7444", &fp(), &secret()).replacen("wfx1.", "wfx2.", 1);
        let err = NodeKey::parse(&key).unwrap_err();
        assert!(err.to_string().contains("not a WireFox node key"), "{err}");
    }

    #[test]
    fn rejects_bad_base64() {
        let err = NodeKey::parse("wfx1.not*base64!").unwrap_err();
        assert!(err.to_string().contains("base64"), "{err}");
        // Standard base64 padding is not part of the format.
        let padded = format!("{}==", encode("panel:7444", &fp(), &secret()));
        assert!(NodeKey::parse(&padded).is_err());
    }

    #[test]
    fn rejects_bad_json() {
        for json in ["not json", r#"{"panel":"p:1","fp":"x"}"#, r#"{"panel":"p:1","fp":"x","secret":"y","z":1}"#] {
            let key = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(json));
            let err = NodeKey::parse(&key).unwrap_err();
            assert!(err.to_string().contains("JSON"), "{json}: {err}");
        }
    }

    #[test]
    fn rejects_bad_fields() {
        let err = NodeKey::parse(&encode("panel:7444", "abcd", &secret())).unwrap_err();
        assert!(err.to_string().contains("fingerprint"), "{err}");
        assert!(NodeKey::parse(&encode("panel:7444", &"zz".repeat(32), &secret())).is_err());
        assert!(NodeKey::parse(&encode("panel:7444", &fp(), "short")).is_err());
        assert!(NodeKey::parse(&encode("panel", &fp(), &secret())).is_err());
        assert!(NodeKey::parse(&encode(":7444", &fp(), &secret())).is_err());
    }
}
