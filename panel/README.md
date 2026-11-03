# wirefox-panel

A web panel for a WireFox deployment. It runs on the same server as the
WireFox server (the core) and lets you manage everything from a browser:

- **The core**: its configuration, its systemd service (start, stop, restart)
  and its logs. Load balancing and timeouts change live; listener changes
  need a restart, which the panel tells you about.
- **Exit-nodes**: create a node, put its key in the node's environment
  (`WIREFOX_NODE_KEY`), run `wirefox-node run` and you're done. The node keeps
  a gRPC connection open to the
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

The web UI listens on `127.0.0.1:8080` over plain HTTP by default, so only this
machine reaches it. Pick one way to reach it from elsewhere:

- an SSH tunnel (`ssh -L 8080:127.0.0.1:8080 server`), then open
  `http://127.0.0.1:8080`. Set the agent address in **Settings** before adding
  nodes, since the panel can't derive it from `127.0.0.1`;
- `-tls` (the panel's own self-signed certificate) or `-tls-cert`/`-tls-key`,
  together with `-addr :8080`;
- a reverse proxy with TLS in front of `127.0.0.1:8080`. Start the panel with
  `-trusted-proxies 127.0.0.1/32` (the proxy's address) so it takes the client
  address, host and scheme from the proxy's headers; without that, every
  login looks like it came from the proxy and shares one rate limit. For
  nginx:

  ```nginx
  location / {
      proxy_pass http://127.0.0.1:8080;
      proxy_set_header Host $host;
      proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
      proxy_set_header X-Forwarded-Proto $scheme;
  }
  ```

The panel logs a warning when it serves plain HTTP on anything but a loopback
address: passwords, sessions and node keys would cross the network in clear.

Ports to open on the server: the panel's agent port (`50051`) for nodes, the
tunnel port (`7443`) for nodes, and the SOCKS ports (`1080`, `1443`) for users.

## Adding a node

1. In **Settings**, set the *agent address*: the `host:port` where nodes reach
   the panel (for example `panel.example.com:50051`). It goes into every key.
2. In **Core**, set the *tunnel public address* if nodes reach the core under
   a different name than the server's listen address.
3. In **Nodes**, click *Add node*. The key is shown once, with ready-made setup
   steps for Linux (systemd), Windows and OpenWrt.
4. On the node, give `wirefox-node run` the key in `WIREFOX_NODE_KEY`, never on
   its command line, where other users could read it. On Linux that's
   `/etc/wirefox/node.env` (mode 600) with `WIREFOX_NODE_KEY=<KEY>`, loaded by
   `examples/wirefox-node-managed.service`.

The node saves the settings it receives under `/var/lib/wirefox-node`
(`%ProgramData%\wirefox-node` on Windows, `/etc/wirefox-node` in the OpenWrt
steps), so it brings its tunnel up right away after a reboot, even while the
panel is unreachable.

A leaked key can be replaced from the node's menu (*New key*). That revokes the
old key and the node's tunnel token, which the old key gave access to, and
drops the agent connected with it. The node's tunnel stays down until it runs
with the new key.

## Flags

| Flag | Default | |
|---|---|---|
| `-addr` | `127.0.0.1:8080` | Web UI listen address |
| `-trusted-proxies` | | Comma-separated CIDRs of reverse proxies whose `X-Forwarded-For`, `X-Forwarded-Host` and `X-Forwarded-Proto` are believed |
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
