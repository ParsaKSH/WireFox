//! Generates the gRPC client and server for the panel's agent protocol. The
//! proto is compiled with protox, so building doesn't need protoc installed.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const PROTO_ROOT: &str = "../../proto";
    const PROTO: &str = "wirefox/agent/v1/agent.proto";
    println!("cargo:rerun-if-changed={PROTO_ROOT}/{PROTO}");

    let fds = protox::compile([PROTO], [PROTO_ROOT])?;
    // The node persists the last TunnelConfig it received as JSON.
    let serde = "#[derive(serde::Serialize, serde::Deserialize)] #[serde(default)]";
    tonic_prost_build::configure()
        // The transport helper `AgentClient::connect(endpoint)` would clash with
        // the `Connect` rpc; the node builds its channel itself anyway.
        .build_transport(false)
        .type_attribute(".wirefox.agent.v1.TunnelConfig", serde)
        .type_attribute(".wirefox.agent.v1.Egress", serde)
        .compile_fds(fds)?;
    Ok(())
}
