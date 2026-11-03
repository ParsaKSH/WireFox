# wirefox-panel HTTP API

The web UI talks to the panel through this JSON API. Everything lives under
`/api`. Except for `POST /api/login`, every endpoint needs the session cookie
set by login (`wirefox_session`, HttpOnly, SameSite=Strict). Errors look like
`{"error": "message"}` with a 4xx/5xx status; a 401 means the session is gone.

Timestamps are Unix seconds unless the field name ends in `_ms`.
Byte counters are plain integers.

Requests other than GET, HEAD and OPTIONS must send their body (if any) as
`Content-Type: application/json` (else 415 `request body must be
application/json`), and an `Origin` header, when the browser sends one, must
match the host the panel was reached at (else 403 `cross-origin request
refused`). Behind a proxy listed in `-trusted-proxies`, that host is taken
from `X-Forwarded-Host` when present.

## Auth

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/login` | `{"username", "password"}` | `{"username"}` + cookie. 429 after too many failures. |
| POST | `/api/logout` | | `{"ok": true}` |
| GET | `/api/me` | | `{"username"}` |
| POST | `/api/password` | `{"current", "new"}` | `{"ok": true}` (new: 8 to 72 bytes) |

A client gets 10 login attempts per 5 minutes; an attempt counts as failed
until it succeeds, and a success clears the count. IPv6 clients are counted
per /64. Logins also answer 429 while too many run at once. The session
cookie is `Secure` when the browser reached the panel over HTTPS (directly, or
through a trusted proxy that sends `X-Forwarded-Proto: https`).

## Overview

`GET /api/overview`

```json
{
  "core": {
    "reachable": true,
    "version": "0.1.0",
    "uptime_secs": 3600,
    "nodes_total": 3,
    "nodes_online": 2,
    "users_total": 5,
    "active_connections": 12,
    "bytes_up": 123456,
    "bytes_down": 7890123,
    "default_balance": "least-load"
  },
  "unit": { "active_state": "active", "sub_state": "running" },
  "agents": { "total": 3, "connected": 2 },
  "history": [
    { "t": 1793600000, "up_bps": 1200.5, "down_bps": 98000.0, "connections": 12 }
  ],
  "events": [ /* the 10 most recent, see /api/events */ ]
}
```

`core` is `null` when the core API can't be reached. `unit` is `null` when the
panel doesn't manage the core through systemd. `history` holds one sample per
5 seconds for the last hour (rates are bytes per second, computed by the panel
from the core's counters).

## Nodes

A node exists both in the WireFox server (tunnel identity, traffic) and in the
panel (agent key, tunnel settings pushed over gRPC). The panel merges both:

```json
{
  "id": "office-1",
  "name": "Office exit",
  "enabled": true,
  "created_at": 1793600000,
  "core": {
    "online": true,
    "rtt_ms": 12.4,
    "idle_pool": 16,
    "remote": "203.0.113.7:51234",
    "connected_at": 1793600100,
    "traffic": { "bytes_up": 1, "bytes_down": 2, "active_connections": 3, "total_connections": 4 },
    "egress_override": null,
    "active_egress": { "interface": "eth1", "source_ip": "" },
    "egress_error": ""
  },
  "agent": {
    "connected": true,
    "last_seen": 1793600200,
    "remote": "203.0.113.7:40000",
    "version": "0.1.0",
    "os": "linux",
    "arch": "aarch64",
    "hostname": "router",
    "tunnel_state": "connected",
    "last_error": "",
    "config_version": 1793600000007,
    "uptime_secs": 5000,
    "connected_secs": 4200,
    "active_streams": 3,
    "total_streams": 812,
    "interfaces": [
      { "name": "eth1", "adapter_name": "", "index": 3, "up": true, "addrs": ["192.0.2.10"] }
    ]
  },
  "config": {
    "version": 1793600000007,
    "pool_size": 16,
    "max_extra": 1024,
    "connect_timeout_secs": 10,
    "allow_local": false,
    "egress": { "interface": "eth1", "source_ip": "" },
    "uplink": { "interface": "", "source_ip": "" }
  },
  "key_created_at": 1793600000
}
```

`core.online` is false (and the session fields empty) when the node's tunnel
isn't connected to the WireFox server. `core.egress_override` is an egress set
for the node through the core's own API; it beats `config.egress`, so the
panel clears it when it takes over a node created in the core directly and
whenever `egress` is saved. `core.active_egress` (`null` while offline) is the
egress the node reports using, and `core.egress_error` why it can't use the
one it was given.

`agent` is `null` when the node has never connected to the panel;
`agent.connected` is false when it did but isn't connected now (the other
fields then show the last known values). `tunnel_state` is one of `stopped`,
`connecting`, `connected`, `error`; `last_error` also carries egress problems
while the tunnel is connected. `connected_secs` is how long the node's tunnel
has been connected, 0 when it isn't.

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/nodes` | | `[Node]` sorted by id |
| POST | `/api/nodes` | `{"id"?, "name"?}` | `{"node": Node, "key": "wfx1...."}` |
| GET | `/api/nodes/{id}` | | `Node` |
| PATCH | `/api/nodes/{id}` | any of `name`, `enabled`, `pool_size`, `max_extra`, `connect_timeout_secs`, `allow_local`, `egress`, `uplink` | `Node` |
| DELETE | `/api/nodes/{id}` | | `{"ok": true}` |
| POST | `/api/nodes/{id}/key` | | `{"key": "wfx1...."}` — new credentials, see below |
| POST | `/api/nodes/{id}/restart` | | `{"ok": true}` — restart the node's tunnel |
| POST | `/api/nodes/{id}/interfaces` | | `{"interfaces": [Interface]}` — fresh list from the node |
| GET | `/api/nodes/{id}/logs?lines=200` | | `{"lines": [{"unix_ms", "level", "message"}]}` (`lines` at most 5000) |
| POST | `/api/nodes/{id}/reset-traffic` | | `Node` |

Node ids made of dots only (`.`, `..`) are refused with 400. Creating a node or
a key answers 409 while the panel's agent address isn't set and the host the
browser used can't be reached by nodes (`localhost`, a loopback or link-local
address, as through an SSH tunnel); set `agent_public_addr` first. Otherwise
the first key saves that host as `agent_public_addr`.

A new key revokes everything the old one gave access to: the agent key and the
node's tunnel token in the WireFox server are both replaced, and the agent
connected with the old key is dropped. The node's tunnel stays down until it
runs with the new key. The node must still exist in the WireFox server (409
otherwise).

Commands that go to the node (`restart`, `interfaces`, `logs`) answer 409 when
the node's agent isn't connected and 504 when it doesn't answer within 10 s.
Any change to the tunnel settings bumps `config.version` and is pushed to the
node right away (or when it next connects). A new node's first version is above
every version the panel handed out before (it starts from the clock in
milliseconds), so a node never mistakes a new config for an old one.

## Users

SOCKS accounts. These are stored in the WireFox server; the panel passes them
through.

```json
{
  "username": "alice",
  "enabled": true,
  "nodes": [],
  "max_connections": null,
  "note": null,
  "balance": null,
  "created_at": 1793600000,
  "traffic": { "bytes_up": 1, "bytes_down": 2, "active_connections": 0, "total_connections": 9 }
}
```

`nodes: []` means any node. `balance` is `round-robin`, `least-ping`,
`least-load`, `random`, or `null` for the server default. Usernames made of
dots only are refused with 400.

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/users` | | `[User]` |
| POST | `/api/users` | `{"username", "password"?, "enabled"?, "nodes"?, "max_connections"?, "note"?, "balance"?}` | `{"user": User, "password"?: "..."}` (password only when generated) |
| PATCH | `/api/users/{name}` | any of `password`, `enabled`, `nodes`, `max_connections`, `note`, `balance` (`null` clears the last three) | `User` |
| DELETE | `/api/users/{name}` | | `{"ok": true}` |
| POST | `/api/users/{name}/reset-traffic` | | `User` |

## Connections

| Method | Path | Response |
|---|---|---|
| GET | `/api/connections?user=&node=` | `[{"id", "user", "node", "target", "client", "started_at", "bytes_up", "bytes_down"}]` |
| DELETE | `/api/connections/{id}` | `{"ok": true}` |

## Core (the WireFox server)

`GET /api/core`

```json
{
  "mode": "systemd",
  "unit": {
    "name": "wirefox-server",
    "active_state": "active",
    "sub_state": "running",
    "main_pid": 1234,
    "since": 1793600000
  },
  "reachable": true,
  "fingerprint": "ab12...",
  "config": {
    "tunnel_listen": "0.0.0.0:7443",
    "tunnel_public_addr": "relay.example.com:7443",
    "socks_listen": ["0.0.0.0:1080"],
    "socks_tls_listen": ["0.0.0.0:1443"],
    "handshake_timeout_secs": 10,
    "connect_timeout_secs": 15,
    "balance": "least-load",
    "tls_sans": ["wirefox"]
  },
  "paths": {
    "config": "/etc/wirefox/server.toml",
    "unit": "/etc/systemd/system/wirefox-server.service",
    "binary": "/usr/local/bin/wirefox-server",
    "data": "/var/lib/wirefox"
  }
}
```

`mode` is `systemd` or `external` (the panel writes the config but someone
else runs the core; `unit` is then `null` and start/stop/restart/logs answer
501). `fingerprint` is empty while the core is unreachable.

| Method | Path | Body | Response |
|---|---|---|---|
| PUT | `/api/core/config` | the `config` object above (all fields) | `{"restart_required": bool}` |
| POST | `/api/core/start` / `stop` / `restart` | | `{"ok": true}` |
| GET | `/api/core/logs?lines=200` | | `{"entries": [{"unix_ms", "priority", "message"}]}` (`lines` at most 5000) |

`PUT /api/core/config` always rewrites the core's config file. `balance` and
the two timeouts are also applied to the running core at once; changing any
other field needs a restart, reported as `restart_required: true` (the panel
does not restart on its own). Nodes keep dialing the tunnel address the running
core listens on: a new `tunnel_listen` or `tunnel_public_addr` is pushed to them
once the core restarts, through `POST /api/core/restart` or (noticed from its
uptime) any other way.

`priority` in log entries is the syslog level (0 emerg … 7 debug). The core
logs plain text, so the panel reads it from the level at the start of the
message (`ERROR` 3, `WARN` 4, `INFO` 6, `DEBUG`/`TRACE` 7); lines without one
keep the journal's priority.

## Settings

`GET /api/settings`, `PUT /api/settings` with

```json
{
  "agent_public_addr": "panel.example.com:50051",
  "socks_public_host": "relay.example.com"
}
```

`agent_public_addr` is written into node keys: it's where nodes reach the
panel's gRPC listener. While the core's `tunnel_public_addr` is empty and it
listens on an unspecified address, nodes also dial the core's tunnel at this
host. `socks_public_host` is only used to show ready-made `socks5://`
addresses for users.

## Events

`GET /api/events?limit=100` returns the activity log, newest first (`limit` at
most 5000, which is also how many events the panel keeps):

```json
[{ "id": 1, "ts": 1793600000, "level": "info", "message": "node office-1 connected to the panel from 203.0.113.7:40000" }]
```

`level` is `info`, `warn` or `error`.
