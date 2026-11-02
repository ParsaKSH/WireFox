//! WireFox exit-node. It keeps a reverse tunnel open to the server and makes
//! outbound connections on the server's behalf through a chosen interface.

pub mod config;
pub mod egress;
pub mod tunnel;

pub use config::NodeConfig;
pub use tunnel::Node;
