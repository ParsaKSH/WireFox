import type {
  Connection,
  CoreConfig,
  CoreInfo,
  CoreLogEntry,
  CreatedNode,
  CreatedUser,
  Me,
  NetInterface,
  Node,
  NodeLogLine,
  NodePatch,
  Ok,
  Overview,
  PanelEvent,
  Settings,
  User,
  UserCreate,
  UserPatch,
} from "./types";

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

let onUnauthorized: (() => void) | null = null;

// The app shell registers this to fall back to the login screen whenever the
// session expires, whichever request notices it first.
export function setUnauthorizedHandler(handler: (() => void) | null) {
  onUnauthorized = handler;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`/api${path}`, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError(0, "the panel is unreachable");
  }

  const text = await res.text();
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
  }

  if (!res.ok) {
    // The login form handles its own 401 (wrong password) without dropping the session.
    if (res.status === 401 && path !== "/login") onUnauthorized?.();
    const message =
      data && typeof data === "object" && "error" in data && typeof data.error === "string"
        ? data.error
        : `${res.status} ${res.statusText || "request failed"}`;
    throw new ApiError(res.status, message);
  }
  return data as T;
}

const get = <T>(path: string) => request<T>("GET", path);
const post = <T>(path: string, body?: unknown) => request<T>("POST", path, body);
const put = <T>(path: string, body: unknown) => request<T>("PUT", path, body);
const patch = <T>(path: string, body: unknown) => request<T>("PATCH", path, body);
const del = <T>(path: string) => request<T>("DELETE", path);

const seg = encodeURIComponent;

function query(params: Record<string, string | number | undefined>): string {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== "") q.set(k, String(v));
  }
  const s = q.toString();
  return s ? `?${s}` : "";
}

export const api = {
  login: (username: string, password: string) => post<Me>("/login", { username, password }),
  logout: () => post<Ok>("/logout"),
  me: () => get<Me>("/me"),
  changePassword: (current: string, next: string) => post<Ok>("/password", { current, new: next }),

  overview: () => get<Overview>("/overview"),

  nodes: () => get<Node[]>("/nodes"),
  node: (id: string) => get<Node>(`/nodes/${seg(id)}`),
  createNode: (body: { id?: string; name?: string }) => post<CreatedNode>("/nodes", body),
  updateNode: (id: string, body: NodePatch) => patch<Node>(`/nodes/${seg(id)}`, body),
  deleteNode: (id: string) => del<Ok>(`/nodes/${seg(id)}`),
  rotateNodeKey: (id: string) => post<{ key: string }>(`/nodes/${seg(id)}/key`),
  restartNode: (id: string) => post<Ok>(`/nodes/${seg(id)}/restart`),
  nodeInterfaces: (id: string) => post<{ interfaces: NetInterface[] }>(`/nodes/${seg(id)}/interfaces`),
  nodeLogs: (id: string, lines: number) => get<{ lines: NodeLogLine[] }>(`/nodes/${seg(id)}/logs${query({ lines })}`),
  resetNodeTraffic: (id: string) => post<Node>(`/nodes/${seg(id)}/reset-traffic`),

  users: () => get<User[]>("/users"),
  createUser: (body: UserCreate) => post<CreatedUser>("/users", body),
  updateUser: (name: string, body: UserPatch) => patch<User>(`/users/${seg(name)}`, body),
  deleteUser: (name: string) => del<Ok>(`/users/${seg(name)}`),
  resetUserTraffic: (name: string) => post<User>(`/users/${seg(name)}/reset-traffic`),

  connections: (filter: { user?: string; node?: string }) => get<Connection[]>(`/connections${query(filter)}`),
  killConnection: (id: number) => del<Ok>(`/connections/${id}`),

  core: () => get<CoreInfo>("/core"),
  saveCoreConfig: (config: CoreConfig) => put<{ restart_required: boolean }>("/core/config", config),
  coreAction: (action: "start" | "stop" | "restart") => post<Ok>(`/core/${action}`),
  coreLogs: (lines: number) => get<{ entries: CoreLogEntry[] }>(`/core/logs${query({ lines })}`),

  settings: () => get<Settings>("/settings"),
  saveSettings: (s: Settings) => put<unknown>("/settings", s),

  events: (limit: number) => get<PanelEvent[]>(`/events${query({ limit })}`),
};

export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
