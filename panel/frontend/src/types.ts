// Shapes of the panel HTTP API (panel/API.md). Timestamps are Unix seconds
// unless the field name ends in _ms.

export type Balance = "round-robin" | "least-ping" | "least-load" | "random";

export const BALANCES: { value: Balance; label: string; hint: string }[] = [
  { value: "least-load", label: "Least load", hint: "fewest active connections" },
  { value: "least-ping", label: "Least ping", hint: "lowest tunnel round-trip time" },
  { value: "round-robin", label: "Round robin", hint: "rotate through the nodes" },
  { value: "random", label: "Random", hint: "pick any online node" },
];

export interface Me {
  username: string;
}

export interface Ok {
  ok: true;
}

export interface Traffic {
  bytes_up: number;
  bytes_down: number;
  active_connections: number;
  total_connections: number;
}

export interface CoreSummary {
  reachable: boolean;
  version: string;
  uptime_secs: number;
  nodes_total: number;
  nodes_online: number;
  users_total: number;
  active_connections: number;
  bytes_up: number;
  bytes_down: number;
  default_balance: Balance;
}

export interface UnitState {
  active_state: string;
  sub_state: string;
}

export interface HistorySample {
  t: number;
  up_bps: number;
  down_bps: number;
  connections: number;
}

export type EventLevel = "info" | "warn" | "error";

export interface PanelEvent {
  id: number;
  ts: number;
  level: EventLevel;
  message: string;
}

export interface Overview {
  core: CoreSummary | null;
  unit: UnitState | null;
  agents: { total: number; connected: number };
  history: HistorySample[];
  events: PanelEvent[];
}

export interface NetInterface {
  name: string;
  adapter_name: string;
  index: number;
  up: boolean;
  addrs: string[];
}

export type TunnelState = "stopped" | "connecting" | "connected" | "error";

export interface NodeCore {
  online: boolean;
  rtt_ms: number;
  idle_pool: number;
  remote: string;
  connected_at: number;
  traffic: Traffic;
  /** Egress set on the WireFox server itself; the panel clears it. */
  egress_override: Binding | null;
  /** What the node reports using; null while offline. */
  active_egress: Binding | null;
  egress_error: string;
}

export interface NodeAgent {
  connected: boolean;
  last_seen: number;
  remote: string;
  version: string;
  os: string;
  arch: string;
  hostname: string;
  tunnel_state: TunnelState;
  last_error: string;
  config_version: number;
  uptime_secs: number;
  // Time since the node's tunnel connected; 0 while it is not connected.
  connected_secs: number;
  active_streams: number;
  total_streams: number;
  interfaces: NetInterface[];
}

export interface Binding {
  interface: string;
  source_ip: string;
}

export interface NodeConfig {
  version: number;
  pool_size: number;
  max_extra: number;
  connect_timeout_secs: number;
  allow_local: boolean;
  egress: Binding;
  uplink: Binding;
}

export interface Node {
  id: string;
  name: string;
  enabled: boolean;
  created_at: number;
  core: NodeCore;
  agent: NodeAgent | null;
  config: NodeConfig;
  key_created_at: number;
}

export interface NodePatch {
  name?: string;
  enabled?: boolean;
  pool_size?: number;
  max_extra?: number;
  connect_timeout_secs?: number;
  allow_local?: boolean;
  egress?: Binding;
  uplink?: Binding;
}

export interface CreatedNode {
  node: Node;
  key: string;
}

export interface NodeLogLine {
  unix_ms: number;
  level: string;
  message: string;
}

export interface User {
  username: string;
  enabled: boolean;
  nodes: string[];
  max_connections: number | null;
  note: string | null;
  balance: Balance | null;
  created_at: number;
  traffic: Traffic;
}

export interface UserCreate {
  username: string;
  password?: string;
  enabled?: boolean;
  nodes?: string[];
  max_connections?: number | null;
  note?: string | null;
  balance?: Balance | null;
}

export type UserPatch = Partial<Omit<UserCreate, "username">>;

export interface CreatedUser {
  user: User;
  password?: string;
}

export interface Connection {
  id: number;
  user: string;
  node: string;
  target: string;
  client: string;
  started_at: number;
  bytes_up: number;
  bytes_down: number;
}

export type CoreMode = "systemd" | "external";

export interface CoreUnit {
  name: string;
  active_state: string;
  sub_state: string;
  main_pid: number;
  since: number;
}

export interface CoreConfig {
  tunnel_listen: string;
  tunnel_public_addr: string;
  socks_listen: string[];
  socks_tls_listen: string[];
  handshake_timeout_secs: number;
  connect_timeout_secs: number;
  balance: Balance;
  tls_sans: string[];
}

export interface CoreInfo {
  mode: CoreMode;
  unit: CoreUnit | null;
  reachable: boolean;
  fingerprint: string;
  config: CoreConfig;
  paths: { config: string; unit: string; binary: string; data: string };
}

export interface CoreLogEntry {
  unix_ms: number;
  priority: number;
  message: string;
}

export interface Settings {
  agent_public_addr: string;
  socks_public_host: string;
}
