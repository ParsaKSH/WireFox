import type { EventLevel, NodeAgent, TunnelState, UnitState } from "./types";
import type { Tone } from "./ui";

export function unitTone(state: string | undefined): Tone {
  switch (state) {
    case "active":
      return "green";
    case "activating":
    case "deactivating":
    case "reloading":
      return "amber";
    case "failed":
      return "red";
    default:
      return "slate";
  }
}

// What the sidebar and dashboard say about the core: the systemd unit when the
// panel manages it, otherwise just whether its API answers.
export function coreState(unit: UnitState | null, reachable: boolean): { label: string; tone: Tone } {
  if (unit) {
    const label = unit.sub_state ? `${unit.active_state} (${unit.sub_state})` : unit.active_state;
    if (unit.active_state === "active" && !reachable) return { label: `${label}, API not answering`, tone: "amber" };
    return { label, tone: unitTone(unit.active_state) };
  }
  return reachable ? { label: "reachable", tone: "green" } : { label: "unreachable", tone: "red" };
}

export const TUNNEL_TONES: Record<TunnelState, Tone> = {
  connected: "green",
  connecting: "amber",
  stopped: "slate",
  error: "red",
};

export function agentState(agent: NodeAgent | null): { label: string; tone: Tone } {
  if (!agent) return { label: "never connected", tone: "slate" };
  if (!agent.connected) return { label: "agent offline", tone: "red" };
  return { label: "agent connected", tone: "green" };
}

export const EVENT_TONES: Record<EventLevel, Tone> = {
  info: "sky",
  warn: "amber",
  error: "red",
};

// Node log levels come from the node's tracing output.
export function logLevelClass(level: string): string {
  switch (level.toLowerCase()) {
    case "error":
      return "text-rose-400";
    case "warn":
    case "warning":
      return "text-amber-300";
    case "info":
      return "text-sky-300";
    case "debug":
      return "text-slate-400";
    default:
      return "text-slate-500";
  }
}

// Syslog priorities: 0 emerg .. 3 err, 4 warning, 5 notice, 6 info, 7 debug.
export function priorityClass(priority: number): string {
  if (priority <= 3) return "text-rose-400";
  if (priority === 4) return "text-amber-300";
  if (priority === 5) return "text-sky-300";
  if (priority === 6) return "text-slate-300";
  return "text-slate-500";
}

export const PRIORITY_NAMES = ["emerg", "alert", "crit", "err", "warning", "notice", "info", "debug"];
