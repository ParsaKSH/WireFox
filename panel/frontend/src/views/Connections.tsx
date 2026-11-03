import { useCallback, useState } from "react";
import { Cable, Pause, Play, RefreshCw, X } from "lucide-react";
import { api, errorMessage } from "../api";
import { useResource } from "../hooks";
import type { Connection } from "../types";
import {
  Badge,
  EmptyState,
  ErrorState,
  fmtBytes,
  fmtDateTime,
  fmtDuration,
  IconButton,
  LoadingState,
  nowSecs,
  PageHeader,
  Select,
  Spinner,
  TableCard,
  useToast,
} from "../ui";

export function Connections() {
  const toast = useToast();
  const [user, setUser] = useState("");
  const [node, setNode] = useState("");
  const [paused, setPaused] = useState(false);
  const [killing, setKilling] = useState<number | null>(null);

  const load = useCallback(() => api.connections({ user, node }), [user, node]);
  const conns = useResource(load, paused ? undefined : 3000);
  // Filter choices come from the full lists, not from what the filter left over.
  const users = useResource(api.users);
  const nodes = useResource(api.nodes);

  const kill = async (c: Connection) => {
    setKilling(c.id);
    try {
      await api.killConnection(c.id);
      if (conns.data) conns.set(conns.data.filter((x) => x.id !== c.id));
      toast("success", `Closed connection to ${c.target}`);
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setKilling(null);
    }
  };

  const list = conns.data ?? [];
  const now = nowSecs();

  return (
    <div>
      <PageHeader
        title="Connections"
        subtitle={
          conns.data
            ? `${list.length} open SOCKS ${list.length === 1 ? "session" : "sessions"}${paused ? " · paused" : " · refreshing every 3 s"}`
            : "Open SOCKS sessions"
        }
        actions={
          <>
            <Select
              aria-label="Filter by user"
              value={user}
              onChange={setUser}
              options={[
                { value: "", label: "All users" },
                ...(users.data ?? []).map((u) => ({ value: u.username, label: u.username })),
              ]}
            />
            <Select
              aria-label="Filter by node"
              value={node}
              onChange={setNode}
              options={[
                { value: "", label: "All nodes" },
                ...(nodes.data ?? []).map((n) => ({ value: n.id, label: n.name ? `${n.name} (${n.id})` : n.id })),
              ]}
            />
            <button className="btn-ghost" onClick={() => setPaused((p) => !p)} aria-pressed={paused}>
              {paused ? <Play className="h-4 w-4" /> : <Pause className="h-4 w-4" />}
              {paused ? "Resume" : "Pause"}
            </button>
            <button className="btn-ghost" onClick={conns.reload} aria-label="Refresh">
              {conns.loading ? <Spinner /> : <RefreshCw className="h-4 w-4" />}
            </button>
          </>
        }
      />

      {!conns.data ? (
        <div className="card">
          {conns.error ? <ErrorState message={conns.error} onRetry={conns.reload} /> : <LoadingState />}
        </div>
      ) : list.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<Cable className="h-10 w-10" />}
            title={user || node ? "No connections match the filter" : "No open connections"}
            hint="Connections show up here while a SOCKS client is using the proxy."
          />
        </div>
      ) : (
        <TableCard head={["User", "Node", "Target", "Client", "Duration", "Down", "Up", ""]}>
          {list.map((c) => (
            <tr key={c.id} className="hover:bg-ink-700/30">
              <td className="td font-medium text-slate-200">{c.user}</td>
              <td className="td">
                <Badge>{c.node}</Badge>
              </td>
              <td className="td max-w-xs truncate font-mono text-xs text-slate-300" title={c.target}>
                {c.target}
              </td>
              <td className="td font-mono text-xs text-slate-400">{c.client}</td>
              <td className="td whitespace-nowrap text-slate-400" title={`since ${fmtDateTime(c.started_at)}`}>
                {fmtDuration(Math.max(0, now - c.started_at))}
              </td>
              <td className="td whitespace-nowrap text-slate-300">{fmtBytes(c.bytes_down)}</td>
              <td className="td whitespace-nowrap text-slate-300">{fmtBytes(c.bytes_up)}</td>
              <td className="td text-right">
                <IconButton label={`Close connection ${c.id}`} danger busy={killing === c.id} onClick={() => kill(c)}>
                  <X className="h-4 w-4" />
                </IconButton>
              </td>
            </tr>
          ))}
        </TableCard>
      )}
      {conns.data && conns.error && <p className="mt-3 text-xs text-amber-300/80">Refresh failed: {conns.error}</p>}
    </div>
  );
}
