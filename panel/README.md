# wirefox-panel

A web panel for a WireFox deployment. It runs on the same server as the
WireFox server (the core) and lets you manage everything from a browser:

- **The core**: its configuration, its systemd service (start, stop, restart)
  and its logs. Load balancing and timeouts change live; listener changes
  need a restart, which the panel tells you about.
- **Exit-nodes**: create a node, copy its key, run `wirefox-node run --key <KEY>`
  on the node and you're done. The node keeps a gRPC connection open to the
  panel; the panel pushes its tunnel settings (pool size, egress interface,
  ...) and can restart its tunnel, list its interfaces and fetch its logs.
- **SOCKS users**, live **connections**, traffic and an activity log.

The panel is not in the data path. If it goes down, the core keeps serving
SOCKS traffic and every node keeps its tunnel with the settings it last
received; nodes reconnect to the panel when it's back.

```
browser ──HTTP(S)──▶ wirefox-panel ──REST (127.0.0.1)──▶ wirefox-server (systemd)
                          ▲  writes server.toml + unit, systemctl, journalctl
                          │
                    gRPC (TLS, pinned)                      ▲ TLS tunnel
                          │                                 │
                     wirefox-node ──────────────────────────┘
```

## Install

The panel manages a systemd service and writes to `/etc`, so it runs as root.

```bash
install -m 755 wirefox-panel wirefox-server /usr/local/bin/
cp examples/wirefox-panel.service /etc/systemd/system/
systemctl daemon-reload
systemctl enable --now wirefox-panel
journalctl -u wirefox-panel | grep -i password   # the generated admin password
```

On first start the panel:

1. creates its data directory (`/var/lib/wirefox-panel`), its admin account and
   a self-signed certificate for the node connections;
2. writes `/etc/wirefox/server.toml` for the core if it doesn't exist (tunnel on
   `:7443`, SOCKS5 on `:1080`, SOCKS5 over TLS on `:1443`, the core API on
   `127.0.0.1:9090` with a key only the panel knows). An existing config is
   kept; the panel only adds its own API key to it;
3. writes `/etc/systemd/system/wirefox-server.service` and enables and starts it.

Set `WIREFOX_PANEL_PASSWORD` in the panel's environment to choose the first
admin password yourself. To reset it later:

```bash
systemctl stop wirefox-panel
WIREFOX_PANEL_PASSWORD='new password' wirefox-panel -reset-password
systemctl start wirefox-panel
```

The web UI listens on `:8080` over plain HTTP by default. Either put it behind a
reverse proxy with TLS, keep it on `127.0.0.1` and reach it through an SSH
tunnel, or start the panel with `-tls` (its own self-signed certificate) or
`-tls-cert`/`-tls-key`.

Ports to open on the server: the panel's agent port (`50051`) for nodes, the
tunnel port (`7443`) for nodes, and the SOCKS ports (`1080`, `1443`) for users.

## Adding a node

1. In **Settings**, set the *agent address*: the `host:port` where nodes reach
   the panel (for example `panel.example.com:50051`). It goes into every key.
2. In **Core**, set the *tunnel public address* if nodes reach the core under
   a different name than the server's listen address.
3. In **Nodes**, click *Add node*. The key is shown once, with ready-made setup
   steps for Linux (systemd), Windows and OpenWrt.
4. On the node: `wirefox-node run --key '<KEY>'` (or put the key in
   `WIREFOX_NODE_KEY`; see `examples/wirefox-node-managed.service`).

The node saves the settings it receives under `/var/lib/wirefox-node`
(`%ProgramData%\wirefox-node` on Windows), so it brings its tunnel up right
away after a reboot, even while the panel is unreachable.

A leaked key can be replaced from the node's menu (*New key*); the old one
stops working immediately.

## Flags

| Flag | Default | |
|---|---|---|
| `-addr` | `:8080` | Web UI listen address |
| `-grpc-addr` | `:50051` | Listen address for node connections |
| `-data` | `/var/lib/wirefox-panel` | Panel database and certificate |
| `-core-mode` | `systemd` | `systemd`: the panel runs the core as a systemd service. `external`: the panel only writes the core's config; you run the core yourself |
| `-core-config` | `/etc/wirefox/server.toml` | The core's config file |
| `-core-bin` | `/usr/local/bin/wirefox-server` | The core's binary, used in the unit file |
| `-core-unit` | `wirefox-server` | The core's systemd unit name |
| `-core-data` | `/var/lib/wirefox` | Data directory for a newly created core config |
| `-admin-user` | `admin` | Admin user name |
| `-reset-password` | | Set a new admin password and exit |
| `-tls` | | Serve the web UI over HTTPS with the panel's certificate |
| `-tls-cert`, `-tls-key` | | Serve the web UI over HTTPS with this certificate |
| `-version` | | Print the version and exit |

## Building

```bash
make build      # frontend (npm) + Go binary in bin/wirefox-panel
make test
make proto      # regenerate internal/agentpb after changing the proto
```

The frontend is embedded into the binary. `make proto` needs `protoc`,
`protoc-gen-go` and `protoc-gen-go-grpc` on `PATH`.

The HTTP API the UI uses is documented in [API.md](API.md); the node protocol in
[`proto/wirefox/agent/v1/agent.proto`](../proto/wirefox/agent/v1/agent.proto).
