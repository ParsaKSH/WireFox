use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use wirefox_node::{egress, Node, NodeConfig};

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(
    name = "wirefox-node",
    version,
    about = "WireFox exit-node: reverse-tunnels to the server and sends traffic out of a chosen interface"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Connect to the server and start forwarding.
    Run {
        /// Path to the config file.
        #[arg(short, long, default_value = "node.toml")]
        config: PathBuf,
    },
    /// List network interfaces (the names usable in `egress.interface`).
    Interfaces,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Interfaces => {
            for i in egress::list_interfaces() {
                let addrs: Vec<String> = i.addrs.iter().map(ToString::to_string).collect();
                let adapter = i.adapter_name.map(|a| format!(" ({a})")).unwrap_or_default();
                let index = i.index.map_or_else(|| "-".into(), |x| x.to_string());
                let state = if i.up { "up" } else { "down" };
                println!("{}{adapter}  index={index}  {state}  {}", i.name, addrs.join(", "));
            }
            Ok(())
        }
        Cmd::Run { config } => {
            wirefox_common::init_tracing("info");
            let cfg = NodeConfig::load(&config)?;
            tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(async {
                let node = Node::new(cfg)?;
                tracing::info!(version = wirefox_common::VERSION, "wirefox-node starting");
                tokio::select! {
                    _ = node.run() => {}
                    _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
                }
                Ok(())
            })
        }
    }
}
