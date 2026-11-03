# wirefox-panel HTTP API

The web UI talks to the panel through this JSON API. Everything lives under
`/api`. Except for `POST /api/login`, every endpoint needs the session cookie
set by login (`wirefox_session`, HttpOnly, SameSite=Strict). Errors look like
`{"error": "message"}` with a 4xx/5xx status; a 401 means the session is gone.

Timestamps are Unix seconds unless the field name ends in `_ms`.
Byte counters are plain integers.

## Auth

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/login` | `{"username", "password"}` | `{"username"}` + cookie. 429 after too many failures. |
| POST | `/api/logout` | | `{"ok": true}` |
| GET | `/api/me` | | `{"username"}` |
| POST | `/api/password` | `{"current", "new"}` | `{"ok": true}` (new: 8+ characters) |

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
    "traffic": { "bytes_up": 1, "bytes_down": 2, "active_connections": 3, "total_connections": 4 }
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
    "config_version": 7,
    "uptime_secs": 5000,
    "active_streams": 3,
    "total_streams": 812,
    "interfaces": [
      { "name": "eth1", "adapter_name": "", "index": 3, "up": true, "addrs": ["192.0.2.10"] }
    ]
  },
  "config": {
    "version": 7,
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
isn't connected to the WireFox server. `agent` is `null` when the node has
never connected to the panel; `agent.connected` is false when it did but isn't
connected now (the other fields then show the last known values).
`tunnel_state` is one of `stopped`, `connecting`, `connected`, `error`.

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/nodes` | | `[Node]` sorted by id |
| POST | `/api/nodes` | `{"id"?, "name"?}` | `{"node": Node, "key": "wfx1...."}` |
| GET | `/api/nodes/{id}` | | `Node` |
| PATCH | `/api/nodes/{id}` | any of `name`, `enabled`, `pool_size`, `max_extra`, `connect_timeout_secs`, `allow_local`, `egress`, `uplink` | `Node` |
| DELETE | `/api/nodes/{id}` | | `{"ok": true}` |
| POST | `/api/nodes/{id}/key` | | `{"key": "wfx1...."}` — a new key; the old one stops working |
| POST | `/api/nodes/{id}/restart` | | `{"ok": true}` — restart the node's tunnel |
| POST | `/api/nodes/{id}/interfaces` | | `{"interfaces": [Interface]}` — fresh list from the node |
| GET | `/api/nodes/{id}/logs?lines=200` | | `{"lines": [{"unix_ms", "level", "message"}]}` |
| POST | `/api/nodes/{id}/reset-traffic` | | `Node` |

Commands that go to the node (`restart`, `interfaces`, `logs`) answer 409 when
the node's agent isn't connected and 504 when it doesn't answer within 10 s.
Any change to the tunnel settings bumps `config.version` and is pushed to the
node right away (or when it next connects).

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
`least-load`, `random`, or `null` for the server default.

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
| GET | `/api/core/logs?lines=200` | | `{"entries": [{"unix_ms", "priority", "message"}]}` |

`PUT /api/core/config` always rewrites the core's config file. `balance` and
the two timeouts are also applied to the running core at once; changing any
other field needs a restart, reported as `restart_required: true` (the panel
does not restart on its own). `priority` in log entries is the syslog level
(0 emerg … 7 debug).

## Settings

`GET /api/settings`, `PUT /api/settings` with

```json
{
  "agent_public_addr": "panel.example.com:50051",
  "socks_public_host": "relay.example.com"
}
```

`agent_public_addr` is written into node keys: it's where nodes reach the
panel's gRPC listener. `socks_public_host` is only used to show ready-made
`socks5://` addresses for users.

## Events

`GET /api/events?limit=100` returns the activity log, newest first:

```json
[{ "id": 1, "ts": 1793600000, "level": "info", "message": "node office-1 connected" }]
```

`level` is `info`, `warn` or `error`.
