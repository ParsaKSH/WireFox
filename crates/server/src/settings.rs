//! SOCKS settings that can be changed through the API while the server runs.
//! They start out as the `[socks]` values from the config file; changes are
//! not written back to it.

use std::ops::RangeInclusive;
use std::time::Duration;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::balance::Strategy;

const TIMEOUT_RANGE: RangeInclusive<u64> = 1..=600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    /// Node selection for users without their own `balance`.
    pub balance: Strategy,
    pub handshake_timeout_secs: u64,
    /// How long a request may wait for a tunnel connection plus the node's dial.
    pub connect_timeout_secs: u64,
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("handshake_timeout_secs", self.handshake_timeout_secs),
            ("connect_timeout_secs", self.connect_timeout_secs),
        ] {
            if !TIMEOUT_RANGE.contains(&value) {
                return Err(format!("{name} must be between {} and {}", TIMEOUT_RANGE.start(), TIMEOUT_RANGE.end()));
            }
        }
        Ok(())
    }

    pub fn handshake_timeout(&self) -> Duration {
        Duration::from_secs(self.handshake_timeout_secs)
    }

    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(self.connect_timeout_secs)
    }
}

/// A partial update; absent fields keep their value.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    pub balance: Option<Strategy>,
    pub handshake_timeout_secs: Option<u64>,
    pub connect_timeout_secs: Option<u64>,
}

/// The current settings, shared by the SOCKS listeners and the API.
pub struct LiveSettings(RwLock<Settings>);

impl LiveSettings {
    pub fn new(initial: Settings) -> Self {
        LiveSettings(RwLock::new(initial))
    }

    pub fn get(&self) -> Settings {
        *self.0.read()
    }

    /// Apply `patch` as a whole or, if the result would be invalid, not at all.
    pub fn apply(&self, patch: SettingsPatch) -> Result<Settings, String> {
        let mut current = self.0.write();
        let next = Settings {
            balance: patch.balance.unwrap_or(current.balance),
            handshake_timeout_secs: patch.handshake_timeout_secs.unwrap_or(current.handshake_timeout_secs),
            connect_timeout_secs: patch.connect_timeout_secs.unwrap_or(current.connect_timeout_secs),
        };
        next.validate()?;
        *current = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> LiveSettings {
        LiveSettings::new(Settings {
            balance: Strategy::LeastLoad,
            handshake_timeout_secs: 10,
            connect_timeout_secs: 15,
        })
    }

    fn patch(json: &str) -> SettingsPatch {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn timeouts_must_be_in_range() {
        let ok = Settings { balance: Strategy::Random, handshake_timeout_secs: 1, connect_timeout_secs: 600 };
        assert!(ok.validate().is_ok());
        for (handshake, connect) in [(0, 15), (10, 0), (601, 15), (10, 601)] {
            let s = Settings { handshake_timeout_secs: handshake, connect_timeout_secs: connect, ..ok };
            assert!(s.validate().is_err(), "{s:?}");
        }
        let err = Settings { connect_timeout_secs: 0, ..ok }.validate().unwrap_err();
        assert!(err.contains("connect_timeout_secs"), "{err}");
    }

    #[test]
    fn patch_changes_only_given_fields() {
        let settings = live();
        let after = settings.apply(patch(r#"{"balance":"round-robin"}"#)).unwrap();
        assert_eq!(
            after,
            Settings { balance: Strategy::RoundRobin, handshake_timeout_secs: 10, connect_timeout_secs: 15 }
        );
        let after = settings.apply(patch(r#"{"handshake_timeout_secs":3,"connect_timeout_secs":30}"#)).unwrap();
        assert_eq!(
            after,
            Settings { balance: Strategy::RoundRobin, handshake_timeout_secs: 3, connect_timeout_secs: 30 }
        );
        assert_eq!(settings.get(), after);
        assert_eq!(settings.apply(SettingsPatch::default()).unwrap(), after);
    }

    #[test]
    fn invalid_patch_changes_nothing() {
        let settings = live();
        let before = settings.get();
        assert!(settings.apply(patch(r#"{"balance":"random","connect_timeout_secs":0}"#)).is_err());
        assert_eq!(settings.get(), before);
    }

    #[test]
    fn patch_rejects_unknown_input() {
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"balance":"fastest"}"#).is_err());
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"connect_timeout_secs":-1}"#).is_err());
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"timeout":5}"#).is_err());
    }
}
