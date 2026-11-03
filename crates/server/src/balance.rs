//! Spreading SOCKS connections over the nodes a user may exit through.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::state::NodeEntry;
use crate::tunnel::NodeSession;

/// How each new connection picks its node.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    /// Take turns, in node id order.
    RoundRobin,
    /// Lowest round-trip time between the server and the node.
    LeastPing,
    /// Fewest active connections.
    #[default]
    LeastLoad,
    Random,
}

pub struct Candidate {
    pub node: Arc<NodeEntry>,
    pub session: Arc<NodeSession>,
}

impl Candidate {
    fn load(&self) -> u64 {
        self.node.stats.active.load(Ordering::Relaxed)
    }

    /// Nodes that haven't answered a ping yet sort last.
    fn rtt(&self) -> u64 {
        match self.session.rtt_us.load(Ordering::Relaxed) {
            0 => u64::MAX,
            rtt => rtt,
        }
    }
}

/// Pick one of `candidates`, which must be sorted by node id so round-robin
/// follows a stable order. `turn` is the caller's round-robin counter.
pub fn choose(strategy: Strategy, mut candidates: Vec<Candidate>, turn: &AtomicUsize) -> Option<Candidate> {
    if candidates.is_empty() {
        return None;
    }
    let index = match strategy {
        Strategy::RoundRobin => turn.fetch_add(1, Ordering::Relaxed) % candidates.len(),
        Strategy::Random => rand::random_range(0..candidates.len()),
        Strategy::LeastLoad => min_index(&candidates, |c| (c.load(), c.rtt())),
        Strategy::LeastPing => min_index(&candidates, |c| (c.rtt(), c.load())),
    };
    Some(candidates.swap_remove(index))
}

fn min_index<K: Ord>(candidates: &[Candidate], key: impl Fn(&Candidate) -> K) -> usize {
    candidates.iter().enumerate().min_by_key(|(_, c)| key(c)).map(|(i, _)| i).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategy_names() {
        for (name, strategy) in [
            ("round-robin", Strategy::RoundRobin),
            ("least-ping", Strategy::LeastPing),
            ("least-load", Strategy::LeastLoad),
            ("random", Strategy::Random),
        ] {
            assert_eq!(serde_json::from_str::<Strategy>(&format!("\"{name}\"")).unwrap(), strategy);
            assert_eq!(serde_json::to_string(&strategy).unwrap(), format!("\"{name}\""));
        }
        assert!(serde_json::from_str::<Strategy>("\"fastest\"").is_err());
    }

    #[test]
    fn empty_has_no_choice() {
        assert!(choose(Strategy::RoundRobin, Vec::new(), &AtomicUsize::new(0)).is_none());
    }
}
