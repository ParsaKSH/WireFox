import { useCallback, useState } from "react";
import { History, RefreshCw } from "lucide-react";
import { api } from "../api";
import { useResource, useTicker } from "../hooks";
import { EVENT_TONES } from "../status";
import type { EventLevel, PanelEvent } from "../types";
import {
  Badge,
  EmptyState,
  ErrorState,
  fmtAgo,
  fmtDateTime,
  LoadingState,
  PageHeader,
  Select,
  Spinner,
  StatusDot,
  TableCard,
} from "../ui";

// Compact list used on the dashboard.
export function EventList({ events }: { events: PanelEvent[] }) {
  if (events.length === 0) {
    return <div className="px-4 py-8 text-center text-sm text-slate-500">No activity yet</div>;
  }
  return (
    <ul className="divide-y divide-ink-600/50">
      {events.map((e) => (
        <li key={e.id} className="flex items-start gap-2.5 px-4 py-2.5 text-sm">
          <span className="mt-1.5">
            <StatusDot tone={EVENT_TONES[e.level]} label={e.level} />
          </span>
          <div className="min-w-0">
            <p className="break-words leading-snug text-slate-300">{e.message}</p>
            <p className="text-[11px] text-slate-600" title={fmtDateTime(e.ts)}>
              {fmtAgo(e.ts)}
            </p>
          </div>
        </li>
      ))}
    </ul>
  );
}

const LIMITS = ["100", "250", "500", "1000"] as const;
type Limit = (typeof LIMITS)[number];
type LevelFilter = EventLevel | "all";

export function ActivityView() {
  useTicker(10000);
  const [limit, setLimit] = useState<Limit>("100");
  const [level, setLevel] = useState<LevelFilter>("all");
  const load = useCallback(() => api.events(Number(limit)), [limit]);
  const events = useResource(load, 10000);
  const { reload } = events;

  const shown = (events.data ?? []).filter((e) => level === "all" || e.level === level);

  return (
    <div>
      <PageHeader
        title="Activity"
        subtitle="What the panel did and saw: node connections, config pushes, core actions"
        actions={
          <>
            <Select<LevelFilter>
              aria-label="Level"
              value={level}
              onChange={setLevel}
              options={[
                { value: "all", label: "All levels" },
                { value: "info", label: "Info" },
                { value: "warn", label: "Warnings" },
                { value: "error", label: "Errors" },
              ]}
            />
            <Select<Limit>
              aria-label="Number of events"
              value={limit}
              onChange={setLimit}
              options={LIMITS.map((l) => ({ value: l, label: `Last ${l}` }))}
            />
            <button className="btn-ghost" onClick={reload} aria-label="Refresh">
              {events.loading ? <Spinner /> : <RefreshCw className="h-4 w-4" />}
            </button>
          </>
        }
      />
      {!events.data ? (
        <div className="card">
          {events.error ? <ErrorState message={events.error} onRetry={reload} /> : <LoadingState />}
        </div>
      ) : shown.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<History className="h-10 w-10" />}
            title={level === "all" ? "No activity yet" : "No events at this level"}
          />
        </div>
      ) : (
        <TableCard head={["Time", "Level", "Message"]}>
          {shown.map((e) => (
            <tr key={e.id} className="hover:bg-ink-700/30">
              <td className="td whitespace-nowrap text-slate-400" title={fmtAgo(e.ts)}>
                {fmtDateTime(e.ts)}
              </td>
              <td className="td">
                <Badge tone={EVENT_TONES[e.level]}>{e.level}</Badge>
              </td>
              <td className="td w-full break-words text-slate-300">{e.message}</td>
            </tr>
          ))}
        </TableCard>
      )}
    </div>
  );
}
