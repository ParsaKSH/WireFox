//! Managed mode: the node takes its tunnel config from wirefox-panel instead of
//! a config file, and keeps a gRPC stream open to the panel so it can be
//! controlled from there. See `proto/wirefox/agent/v1/agent.proto`.

#[cfg(windows)]
mod acl;
mod agent;
mod key;
mod logs;
mod state;
mod supervisor;

pub use agent::Managed;
pub use key::NodeKey;
pub use logs::{LogBuffer, LogLayer};
pub use state::{default_state_dir, StateDir};
