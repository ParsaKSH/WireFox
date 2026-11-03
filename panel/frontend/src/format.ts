export function cls(...xs: (string | false | null | undefined)[]): string {
  return xs.filter(Boolean).join(" ");
}

const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];

export function fmtBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), BYTE_UNITS.length - 1);
  const v = n / 1024 ** i;
  return `${v.toFixed(i === 0 ? 0 : v < 10 ? 2 : 1)} ${BYTE_UNITS[i]}`;
}

export function fmtRate(bytesPerSec: number): string {
  return `${fmtBytes(bytesPerSec)}/s`;
}

export function fmtDuration(secs: number): string {
  if (!Number.isFinite(secs) || secs < 0) return "—";
  const s = Math.floor(secs);
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  if (m) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

export function nowSecs(): number {
  return Math.floor(Date.now() / 1000);
}

export function fmtAgo(unixSecs: number): string {
  if (!unixSecs) return "never";
  const d = nowSecs() - unixSecs;
  if (d < 5) return "just now";
  return `${fmtDuration(d)} ago`;
}

export function fmtDateTime(unixSecs: number): string {
  return unixSecs ? new Date(unixSecs * 1000).toLocaleString() : "—";
}

export function fmtClock(unixMs: number): string {
  return new Date(unixMs).toLocaleTimeString([], { hour12: false });
}
