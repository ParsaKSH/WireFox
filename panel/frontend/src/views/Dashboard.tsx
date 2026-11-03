import type { ReactNode } from "react";
import { ArrowDown, ArrowDownUp, ArrowUp, Cable, Cpu, PlugZap, Server, Users } from "lucide-react";
import type { ViewId } from "../App";
import { fmtBytes, fmtDuration } from "../format";
import type { Resource } from "../hooks";
import { useTicker } from "../hooks";
import { coreState } from "../status";
import type { Overview } from "../types";
import { Banner, ErrorState, LoadingState, PageHeader, SectionTitle, StatusDot, type Tone } from "../ui";
import { EventList } from "./Activity";
import { TrafficChart } from "./TrafficChart";

function Stat({
  icon,
  label,
  value,
  sub,
  tone,
}: {
  icon: ReactNode;
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  tone?: Tone;
}) {
  return (
    <div className="card p-5">
      <div className="flex items-center justify-between">
        <span className="text-sm text-slate-400">{label}</span>
        <span className="text-fox-400">{icon}</span>
      </div>
      <div className="mt-2 flex items-center gap-2 text-2xl font-semibold text-slate-100">
        {tone && <StatusDot tone={tone} pulse={tone === "green"} />}
        <span className="truncate">{value}</span>
      </div>
      {sub && <div className="mt-1 truncate text-xs text-slate-500">{sub}</div>}
    </div>
  );
}

export function Dashboard({ overview, go }: { overview: Resource<Overview>; go: (v: ViewId) => void }) {
  useTicker(5000);
  const o = overview.data;

  if (!o) {
    return (
      <div>
        <PageHeader title="Dashboard" subtitle="Core, exit-nodes and traffic at a glance" />
        <div className="card">
          {overview.error ? <ErrorState message={overview.error} onRetry={overview.reload} /> : <LoadingState />}
        </div>
      </div>
    );
  }

  const core = o.core;
  const state = coreState(o.unit, core?.reachable ?? false);

  return (
    <div>
      <PageHeader title="Dashboard" subtitle="Core, exit-nodes and traffic at a glance" />

      {!core && (
        <div className="mb-6">
          <Banner tone="red">
            The core API is not answering, so node, user and traffic figures are unavailable.{" "}
            <button className="font-medium underline underline-offset-2" onClick={() => go("core")}>
              Check the core
            </button>
          </Banner>
        </div>
      )}

      <div className="mb-6 grid grid-cols-2 gap-4 lg:grid-cols-3">
        <Stat
          icon={<Cpu className="h-5 w-5" />}
          label="Core"
          value={state.label}
          tone={state.tone}
          sub={
            core
              ? `v${core.version} · up ${fmtDuration(core.uptime_secs)}`
              : o.unit
                ? "systemd unit"
                : "managed outside the panel"
          }
        />
        <Stat
          icon={<Server className="h-5 w-5" />}
          label="Nodes online"
          value={core ? `${core.nodes_online}/${core.nodes_total}` : "—"}
          sub="tunnels connected to the core"
        />
        <Stat
          icon={<PlugZap className="h-5 w-5" />}
          label="Agents connected"
          value={`${o.agents.connected}/${o.agents.total}`}
          sub="nodes the panel can control"
        />
        <Stat
          icon={<Users className="h-5 w-5" />}
          label="Users"
          value={core ? core.users_total : "—"}
          sub={core ? `default balance: ${core.default_balance}` : undefined}
        />
        <Stat
          icon={<Cable className="h-5 w-5" />}
          label="Active connections"
          value={core ? core.active_connections : "—"}
          sub="open SOCKS sessions"
        />
        <Stat
          icon={<ArrowDownUp className="h-5 w-5" />}
          label="Total traffic"
          value={
            core ? (
              <span className="flex items-center gap-3 text-xl">
                <span className="flex items-center gap-1">
                  <ArrowDown className="h-4 w-4 text-slate-500" aria-label="download" />
                  {fmtBytes(core.bytes_down)}
                </span>
                <span className="flex items-center gap-1">
                  <ArrowUp className="h-4 w-4 text-slate-500" aria-label="upload" />
                  {fmtBytes(core.bytes_up)}
                </span>
              </span>
            ) : (
              "—"
            )
          }
          sub="since the counters were last reset"
        />
      </div>

      <div className="grid grid-cols-1 gap-6 xl:grid-cols-3">
        <div className="xl:col-span-2">
          <SectionTitle>Traffic, last hour</SectionTitle>
          <div className="card p-5">
            <TrafficChart history={o.history} />
          </div>
        </div>
        <div>
          <SectionTitle
            actions={
              <button className="text-sm text-fox-400 hover:text-fox-300" onClick={() => go("activity")}>
                All activity
              </button>
            }
          >
            Recent events
          </SectionTitle>
          <div className="card">
            <EventList events={o.events} />
          </div>
        </div>
      </div>
    </div>
  );
}
