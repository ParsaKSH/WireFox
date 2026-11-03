import { useEffect, useMemo, useRef, useState, type MouseEvent } from "react";
import { fmtDuration, fmtRate } from "../format";
import type { HistorySample } from "../types";

// Colors checked for lightness, color-blind separation and contrast on the ink surface.
const DOWN = "#ea580c";
const UP = "#0284c7";

const HEIGHT = 240;
const PAD = { top: 12, right: 16, bottom: 26, left: 72 };

// Picks round tick values in binary units (e.g. 0, 256 KiB/s, 512 KiB/s, ...).
export function rateTicks(max: number): number[] {
  if (max <= 0) return [0];
  const unit = 1024 ** Math.max(0, Math.floor(Math.log(max) / Math.log(1024)));
  const scaled = max / unit;
  const raw = scaled / 4;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = ([1, 2, 2.5, 5, 10].find((m) => m * mag >= raw) ?? 10) * mag * unit;
  const ticks: number[] = [];
  for (let v = 0; v <= max + step * 0.001; v += step) ticks.push(v);
  if (ticks[ticks.length - 1] < max) ticks.push(ticks[ticks.length - 1] + step);
  return ticks;
}

// Steps for the time axis, in seconds.
const TIME_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 10800, 21600, 43200, 86400];

// Picks time-axis ticks between t0 and t1 (Unix seconds): at most maxCount of
// them, on round local-clock steps. Labels need seconds when the step is
// shorter than a minute; otherwise HH:MM is already distinct.
export function timeTicks(t0: number, t1: number, maxCount: number): { ticks: number[]; seconds: boolean } {
  const span = Math.max(1, t1 - t0);
  const step = TIME_STEPS.find((s) => span / s < Math.max(2, maxCount)) ?? TIME_STEPS[TIME_STEPS.length - 1];
  // Align to the local clock, so hourly ticks land on :00 in every time zone.
  const shift = -new Date(t0 * 1000).getTimezoneOffset() * 60;
  const ticks: number[] = [];
  for (let t = Math.ceil((t0 + shift) / step) * step - shift; t <= t1; t += step) ticks.push(t);
  return { ticks, seconds: step < 60 };
}

function clock(t: number, seconds = false): string {
  return new Date(t * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: seconds ? "2-digit" : undefined,
    hour12: false,
  });
}

export function TrafficChart({ history }: { history: HistorySample[] }) {
  const boxRef = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [hover, setHover] = useState<number | null>(null);

  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const geo = useMemo(() => {
    if (history.length < 2 || width === 0) return null;
    const innerW = width - PAD.left - PAD.right;
    const innerH = HEIGHT - PAD.top - PAD.bottom;
    const t0 = history[0].t;
    const t1 = history[history.length - 1].t;
    const span = Math.max(1, t1 - t0);
    const peak = history.reduce((m, s) => Math.max(m, s.up_bps, s.down_bps), 0);
    const ticks = rateTicks(peak);
    const top = ticks[ticks.length - 1] || 1;
    const x = (t: number) => PAD.left + ((t - t0) / span) * innerW;
    const y = (v: number) => PAD.top + innerH - (v / top) * innerH;
    const line = (pick: (s: HistorySample) => number) =>
      history.map((s, i) => `${i ? "L" : "M"}${x(s.t).toFixed(1)},${y(pick(s)).toFixed(1)}`).join("");
    const area = (pick: (s: HistorySample) => number) =>
      `${line(pick)}L${x(t1).toFixed(1)},${y(0)}L${x(t0).toFixed(1)},${y(0)}Z`;
    return {
      innerW,
      x,
      y,
      ticks,
      // Roughly one label per 90 px, which fits HH:MM:SS with room to spare.
      time: timeTicks(t0, t1, Math.floor(innerW / 90)),
      downLine: line((s) => s.down_bps),
      downArea: area((s) => s.down_bps),
      upLine: line((s) => s.up_bps),
      upArea: area((s) => s.up_bps),
    };
  }, [history, width]);

  const onMove = (e: MouseEvent<SVGSVGElement>) => {
    if (!geo) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const px = e.clientX - rect.left;
    let best = 0;
    for (let i = 1; i < history.length; i++) {
      if (Math.abs(geo.x(history[i].t) - px) < Math.abs(geo.x(history[best].t) - px)) best = i;
    }
    setHover(best);
  };

  const sample = hover !== null ? history[hover] : null;
  const last = history[history.length - 1];

  return (
    <div>
      <div className="mb-2 flex flex-wrap items-center gap-4 text-xs text-slate-400">
        <span className="flex items-center gap-1.5">
          <span className="h-2 w-3 rounded-sm" style={{ background: DOWN }} /> Download
          {last && <span className="text-slate-200">{fmtRate(last.down_bps)}</span>}
        </span>
        <span className="flex items-center gap-1.5">
          <span className="h-2 w-3 rounded-sm" style={{ background: UP }} /> Upload
          {last && <span className="text-slate-200">{fmtRate(last.up_bps)}</span>}
        </span>
      </div>
      <div ref={boxRef} className="relative" style={{ height: HEIGHT }}>
        {!geo ? (
          <div className="grid h-full place-items-center text-sm text-slate-500">
            {history.length < 2 ? "Collecting samples; the chart fills in over the next minutes." : null}
          </div>
        ) : (
          <svg
            width={width}
            height={HEIGHT}
            role="img"
            aria-label={`Traffic over the last ${fmtDuration(last.t - history[0].t)}. Now ${fmtRate(last.down_bps)} down, ${fmtRate(last.up_bps)} up.`}
            onMouseMove={onMove}
            onMouseLeave={() => setHover(null)}
          >
            {geo.ticks.map((v) => (
              <g key={v}>
                <line
                  x1={PAD.left}
                  x2={PAD.left + geo.innerW}
                  y1={geo.y(v)}
                  y2={geo.y(v)}
                  stroke="#202736"
                  strokeDasharray={v ? "3 4" : undefined}
                />
                <text x={PAD.left - 8} y={geo.y(v)} dy="0.32em" textAnchor="end" className="fill-slate-500 text-[10px]">
                  {fmtRate(v)}
                </text>
              </g>
            ))}
            {geo.time.ticks.map((t) => {
              // Labels near the plot edges are anchored inwards so they aren't clipped.
              const x = geo.x(t);
              const anchor = x < PAD.left + 24 ? "start" : x > PAD.left + geo.innerW - 24 ? "end" : "middle";
              return (
                <text key={t} x={x} y={HEIGHT - 8} textAnchor={anchor} className="fill-slate-500 text-[10px]">
                  {clock(t, geo.time.seconds)}
                </text>
              );
            })}
            <path d={geo.downArea} fill={DOWN} fillOpacity={0.16} />
            <path d={geo.upArea} fill={UP} fillOpacity={0.16} />
            <path d={geo.downLine} fill="none" stroke={DOWN} strokeWidth={2} strokeLinejoin="round" />
            <path d={geo.upLine} fill="none" stroke={UP} strokeWidth={2} strokeLinejoin="round" />
            {sample && (
              <g pointerEvents="none">
                <line
                  x1={geo.x(sample.t)}
                  x2={geo.x(sample.t)}
                  y1={PAD.top}
                  y2={HEIGHT - PAD.bottom}
                  stroke="#94a3b8"
                  strokeOpacity={0.4}
                />
                <circle
                  cx={geo.x(sample.t)}
                  cy={geo.y(sample.down_bps)}
                  r={4}
                  fill={DOWN}
                  stroke="#10141d"
                  strokeWidth={2}
                />
                <circle
                  cx={geo.x(sample.t)}
                  cy={geo.y(sample.up_bps)}
                  r={4}
                  fill={UP}
                  stroke="#10141d"
                  strokeWidth={2}
                />
              </g>
            )}
          </svg>
        )}
        {geo && sample && (
          <div
            className="pointer-events-none absolute top-2 z-10 w-44 rounded-lg border border-ink-500/70 bg-ink-900/95 px-3 py-2 text-xs shadow-glow"
            style={
              geo.x(sample.t) > width / 2 ? { right: width - geo.x(sample.t) + 12 } : { left: geo.x(sample.t) + 12 }
            }
          >
            <div className="mb-1 font-mono text-slate-400">{clock(sample.t, true)}</div>
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-1.5 text-slate-400">
                <span className="h-2 w-2 rounded-full" style={{ background: DOWN }} />
                Down
              </span>
              <span className="text-slate-100">{fmtRate(sample.down_bps)}</span>
            </div>
            <div className="flex items-center justify-between gap-2">
              <span className="flex items-center gap-1.5 text-slate-400">
                <span className="h-2 w-2 rounded-full" style={{ background: UP }} />
                Up
              </span>
              <span className="text-slate-100">{fmtRate(sample.up_bps)}</span>
            </div>
            <div className="flex items-center justify-between gap-2">
              <span className="text-slate-400">Connections</span>
              <span className="text-slate-100">{sample.connections}</span>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
