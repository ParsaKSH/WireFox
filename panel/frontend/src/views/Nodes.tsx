import { useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  KeyRound,
  MoreHorizontal,
  Network,
  Pencil,
  Plus,
  RotateCcw,
  RotateCw,
  ScrollText,
  Server,
  Trash2,
} from "lucide-react";
import { api, errorMessage } from "../api";
import { fmtAgo, fmtBytes, fmtDateTime, fmtDuration } from "../format";
import { POLL_MS, useResource, useTicker } from "../hooks";
import { agentState, TUNNEL_TONES } from "../status";
import type { Binding, Node } from "../types";
import {
  Badge,
  ConfirmDialog,
  EmptyState,
  ErrorState,
  IconButton,
  LoadingState,
  Menu,
  PageHeader,
  StatusDot,
  TableCard,
  useToast,
} from "../ui";
import { AddNodeModal, EditNodeModal, NodeLogsModal } from "./NodeDialogs";
import { NodeKeyModal } from "./NodeInstall";

type Dialog =
  | { kind: "add" }
  | { kind: "edit"; node: Node }
  | { kind: "logs"; node: Node }
  | { kind: "key"; nodeId: string; key: string }
  | { kind: "rotate"; node: Node }
  | { kind: "delete"; node: Node };

const NOT_CONNECTED_HINT = "The node's agent is not connected to the panel";

function bindingLabel(b: Binding): string | null {
  if (!b.interface && !b.source_ip) return null;
  return [b.interface, b.source_ip].filter(Boolean).join(" / ");
}

function AgentCell({ node }: { node: Node }) {
  const a = node.agent;
  const state = agentState(a);
  return (
    <div className="space-y-1">
      <div className="flex items-center gap-2">
        <StatusDot tone={state.tone} pulse={state.tone === "green"} />
        <span className="whitespace-nowrap text-slate-300">{state.label}</span>
      </div>
      {a && (
        <div className="flex items-center gap-1.5 text-xs">
          {a.connected ? (
            <>
              <Badge tone={TUNNEL_TONES[a.tunnel_state]} title={a.last_error || undefined}>
                tunnel {a.tunnel_state}
              </Badge>
              {a.connected_secs > 0 && (
                <span className="whitespace-nowrap text-slate-500" title="Time since the tunnel last connected">
                  for {fmtDuration(a.connected_secs)}
                </span>
              )}
            </>
          ) : (
            <span className="text-slate-500" title={fmtDateTime(a.last_seen)}>
              seen {fmtAgo(a.last_seen)}
            </span>
          )}
        </div>
      )}
      {a?.connected && a.last_error && (
        <p className="max-w-[14rem] truncate text-[11px] text-rose-300/80" title={a.last_error}>
          {a.last_error}
        </p>
      )}
    </div>
  );
}

function CoreCell({ node }: { node: Node }) {
  const c = node.core;
  if (!c.online) {
    return (
      <div className="flex items-center gap-2">
        <StatusDot tone={node.enabled ? "red" : "slate"} />
        <span className="whitespace-nowrap text-slate-400">offline</span>
      </div>
    );
  }
  return (
    <div className="space-y-1">
      <div className="flex items-center gap-2">
        <StatusDot tone="green" pulse />
        <span className="whitespace-nowrap text-slate-300">online</span>
        <span className="font-mono text-xs text-slate-400">{c.rtt_ms.toFixed(1)} ms</span>
      </div>
      <div className="whitespace-nowrap text-xs text-slate-500" title={`connected ${fmtDateTime(c.connected_at)}`}>
        {c.idle_pool} idle · up {fmtDuration(Math.max(0, Date.now() / 1000 - c.connected_at))}
      </div>
    </div>
  );
}

function HostCell({ node }: { node: Node }) {
  const a = node.agent;
  if (!a) return <span className="text-xs text-slate-500">unknown</span>;
  return (
    <div className="space-y-0.5">
      <div className="font-mono text-xs text-slate-300">{a.hostname || "—"}</div>
      <div className="whitespace-nowrap text-xs text-slate-500">
        {a.os}/{a.arch} · v{a.version}
      </div>
    </div>
  );
}

export function Nodes() {
  useTicker(5000);
  const toast = useToast();
  const nodes = useResource(api.nodes, POLL_MS);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  // Mutations finish after an await, so they merge into whatever the list is by
  // then rather than into the one their row was rendered from.
  const replace = (fresh: Node) => nodes.set((cur) => cur.map((n) => (n.id === fresh.id ? fresh : n)));

  // Runs a row action, tracking it per node so its row can show a spinner.
  const act = async (node: Node, what: string, fn: () => Promise<void>) => {
    setBusy(`${node.id}:${what}`);
    try {
      await fn();
    } catch (e) {
      toast("error", `${node.id}: ${errorMessage(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const restartTunnel = (node: Node) =>
    act(node, "restart", async () => {
      await api.restartNode(node.id);
      toast("success", `Tunnel of ${node.id} is restarting`);
      void nodes.reload();
    });

  const refreshInterfaces = (node: Node) =>
    act(node, "interfaces", async () => {
      const { interfaces } = await api.nodeInterfaces(node.id);
      nodes.set((cur) =>
        cur.map((n) => (n.id === node.id && n.agent ? { ...n, agent: { ...n.agent, interfaces } } : n)),
      );
      toast(
        "success",
        `${node.id} reported ${interfaces.length} ${interfaces.length === 1 ? "interface" : "interfaces"}`,
      );
    });

  const resetTraffic = (node: Node) =>
    act(node, "reset", async () => {
      replace(await api.resetNodeTraffic(node.id));
      toast("success", `Traffic counters of ${node.id} reset`);
    });

  const rotateKey = async (node: Node) => {
    try {
      const { key } = await api.rotateNodeKey(node.id);
      setDialog({ kind: "key", nodeId: node.id, key });
    } catch (e) {
      toast("error", errorMessage(e));
      throw e;
    }
  };

  const remove = async (node: Node) => {
    try {
      await api.deleteNode(node.id);
      nodes.set((cur) => cur.filter((n) => n.id !== node.id));
      toast("success", `Node ${node.id} deleted`);
    } catch (e) {
      toast("error", errorMessage(e));
      throw e;
    }
  };

  const list = nodes.data ?? [];
  const addButton = (
    <button className="btn-primary" onClick={() => setDialog({ kind: "add" })}>
      <Plus className="h-4 w-4" /> Add node
    </button>
  );

  return (
    <div>
      <PageHeader
        title="Nodes"
        subtitle="Exit-nodes: their agent link to the panel and their tunnel to the core"
        actions={addButton}
      />

      {!nodes.data ? (
        <div className="card">
          {nodes.error ? <ErrorState message={nodes.error} onRetry={nodes.reload} /> : <LoadingState />}
        </div>
      ) : list.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<Server className="h-10 w-10" />}
            title="No nodes yet"
            hint="An exit-node is a machine that carries SOCKS traffic out to the internet. Add one to get its key and install instructions."
            action={addButton}
          />
        </div>
      ) : (
        <TableCard head={["Node", "Agent", "Core tunnel", "Host", "Active", "Traffic", "Egress", ""]}>
          {list.map((n) => {
            const connected = !!n.agent?.connected;
            const egress = bindingLabel(n.config.egress);
            const pending = n.agent && n.agent.connected && n.agent.config_version < n.config.version;
            return (
              <tr key={n.id} className="hover:bg-ink-700/30">
                <td className="td">
                  <div className="flex items-center gap-2">
                    <span className={n.enabled ? "font-mono font-medium text-slate-100" : "font-mono text-slate-500"}>
                      {n.id}
                    </span>
                    {!n.enabled && <Badge tone="slate">disabled</Badge>}
                  </div>
                  {n.name && <div className="mt-0.5 max-w-[14rem] truncate text-xs text-slate-500">{n.name}</div>}
                  {pending && (
                    <div className="mt-0.5 text-[11px] text-amber-300/80">
                      config v{n.config.version} not applied yet (node at v{n.agent?.config_version})
                    </div>
                  )}
                </td>
                <td className="td">
                  <AgentCell node={n} />
                </td>
                <td className="td">
                  <CoreCell node={n} />
                </td>
                <td className="td">
                  <HostCell node={n} />
                </td>
                <td className="td text-slate-300">{n.core.traffic.active_connections}</td>
                <td className="td whitespace-nowrap text-xs text-slate-300">
                  <div className="flex items-center gap-1">
                    <ArrowDown className="h-3 w-3 text-slate-500" aria-label="download" />
                    {fmtBytes(n.core.traffic.bytes_down)}
                  </div>
                  <div className="flex items-center gap-1">
                    <ArrowUp className="h-3 w-3 text-slate-500" aria-label="upload" />
                    {fmtBytes(n.core.traffic.bytes_up)}
                  </div>
                </td>
                <td className="td">
                  {egress ? (
                    <span className="font-mono text-xs text-slate-300">{egress}</span>
                  ) : (
                    <span className="text-xs text-slate-500">default</span>
                  )}
                  {n.core.egress_error && (
                    <p className="max-w-[12rem] truncate text-[11px] text-rose-300/80" title={n.core.egress_error}>
                      {n.core.egress_error}
                    </p>
                  )}
                </td>
                <td className="td">
                  <div className="flex justify-end gap-0.5">
                    <IconButton label="Edit" onClick={() => setDialog({ kind: "edit", node: n })}>
                      <Pencil className="h-4 w-4" />
                    </IconButton>
                    <IconButton
                      label={connected ? "Logs" : `Logs: ${NOT_CONNECTED_HINT.toLowerCase()}`}
                      disabled={!connected}
                      onClick={() => setDialog({ kind: "logs", node: n })}
                    >
                      <ScrollText className="h-4 w-4" />
                    </IconButton>
                    <Menu
                      label={`More actions for ${n.id}`}
                      icon={<MoreHorizontal className="h-4 w-4" />}
                      items={[
                        {
                          label: "Restart tunnel",
                          icon: <RotateCw className="h-4 w-4" />,
                          disabled: !connected || busy === `${n.id}:restart`,
                          hint: NOT_CONNECTED_HINT,
                          onSelect: () => void restartTunnel(n),
                        },
                        {
                          label: "Refresh interfaces",
                          icon: <Network className="h-4 w-4" />,
                          disabled: !connected || busy === `${n.id}:interfaces`,
                          hint: NOT_CONNECTED_HINT,
                          onSelect: () => void refreshInterfaces(n),
                        },
                        {
                          label: "Reset traffic",
                          icon: <RotateCcw className="h-4 w-4" />,
                          disabled: busy === `${n.id}:reset`,
                          onSelect: () => void resetTraffic(n),
                        },
                        {
                          label: "New key",
                          icon: <KeyRound className="h-4 w-4" />,
                          onSelect: () => setDialog({ kind: "rotate", node: n }),
                        },
                        {
                          label: "Delete",
                          icon: <Trash2 className="h-4 w-4" />,
                          danger: true,
                          onSelect: () => setDialog({ kind: "delete", node: n }),
                        },
                      ]}
                    />
                  </div>
                </td>
              </tr>
            );
          })}
        </TableCard>
      )}
      {nodes.data && nodes.error && <p className="mt-3 text-xs text-amber-300/80">Refresh failed: {nodes.error}</p>}

      {dialog?.kind === "add" && (
        <AddNodeModal
          onClose={() => setDialog(null)}
          onCreated={(node, key) => {
            nodes.set((cur) => [...cur, node].sort((a, b) => a.id.localeCompare(b.id)));
            setDialog({ kind: "key", nodeId: node.id, key });
          }}
        />
      )}
      {dialog?.kind === "edit" && (
        <EditNodeModal
          node={dialog.node}
          onClose={() => setDialog(null)}
          onSaved={(node) => {
            replace(node);
            setDialog(null);
            toast("success", `Node ${node.id} saved`);
          }}
        />
      )}
      {dialog?.kind === "logs" && <NodeLogsModal node={dialog.node} onClose={() => setDialog(null)} />}
      {dialog?.kind === "key" && (
        <NodeKeyModal nodeId={dialog.nodeId} nodeKey={dialog.key} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === "rotate" && (
        <ConfirmDialog
          title={`New key for ${dialog.node.id}`}
          confirmLabel="Issue new key"
          danger
          message={
            <>
              The current key stops working at once and the node disconnects from the panel until it is started with the
              new key. Its tunnel keeps running from its saved settings meanwhile.
            </>
          }
          onConfirm={() => rotateKey(dialog.node)}
          // A successful rotation replaces this dialog with the key; only close when it is still showing.
          onClose={() => setDialog((d) => (d?.kind === "rotate" ? null : d))}
        />
      )}
      {dialog?.kind === "delete" && (
        <ConfirmDialog
          title={`Delete ${dialog.node.id}`}
          confirmLabel="Delete node"
          danger
          typeToConfirm={dialog.node.id}
          message={
            <>
              The node is removed from the panel and the core: its key stops working, its tunnel is cut and its active
              connections are closed. Users restricted to it lose access to it.
            </>
          }
          onConfirm={() => remove(dialog.node)}
          onClose={() => setDialog((d) => (d?.kind === "delete" ? null : d))}
        />
      )}
    </div>
  );
}
