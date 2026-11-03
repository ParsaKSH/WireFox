//! WireFox exit-node. It keeps a reverse tunnel open to the server and makes
//! outbound connections on the server's behalf through a chosen interface.
//!
//! Standalone, the tunnel is described by a config file. In managed mode
//! ([`managed`]) it comes from wirefox-panel instead.

pub mod config;
pub mod egress;
pub mod managed;
pub mod tunnel;

/// Messages and gRPC client/server of the panel's agent protocol.
#[allow(clippy::large_enum_variant)] // Generated: PanelMessage holds a TunnelConfig inline.
pub mod pb {
    tonic::include_proto!("wirefox.agent.v1");
}

pub use config::NodeConfig;
pub use tunnel::{Monitor, Node, TunnelState, TunnelStatus};
