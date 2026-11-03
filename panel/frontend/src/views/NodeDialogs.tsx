import { useCallback, useId, useState, type FormEvent } from "react";
import { RefreshCw } from "lucide-react";
import { api, errorMessage } from "../api";
import { cls, fmtClock } from "../format";
import { useResource } from "../hooks";
import { logLevelClass } from "../status";
import type { Binding, NetInterface, Node, NodePatch } from "../types";
import { ErrorState, Field, IconButton, LoadingState, Modal, Select, Spinner, Toggle, useToast } from "../ui";
import { idError, intError } from "../validate";

export function AddNodeModal({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (n: Node, key: string) => void;
}) {
  const toast = useToast();
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const idErr = idError(id);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      const res = await api.createNode({ id: id || undefined, name: name.trim() || undefined });
      onCreated(res.node, res.key);
    } catch (e) {
      toast("error", errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title="Add node"
      onClose={onClose}
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button type="submit" form="add-node" className="btn-primary" disabled={busy || !!idErr}>
            {busy && <Spinner />} Create and show key
          </button>
        </>
      }
    >
      <form id="add-node" onSubmit={submit} className="space-y-4">
        <Field
          label="ID"
          hint="Short identifier used in user restrictions and logs. Leave empty to have one generated."
          error={idErr}
        >
          {(fid) => (
            <input
              id={fid}
              className="input font-mono"
              value={id}
              placeholder="office-1"
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => setId(e.target.value.trim())}
            />
          )}
        </Field>
        <Field label="Name" hint="Free text shown next to the ID.">
          {(fid) => (
            <input
              id={fid}
              className="input"
              value={name}
              placeholder="Office uplink"
              onChange={(e) => setName(e.target.value)}
            />
          )}
        </Field>
      </form>
    </Modal>
  );
}

const NONE = "";
const CUSTOM = "\u0000custom";

function interfaceLabel(i: NetInterface): string {
  const addrs = i.addrs.length ? i.addrs.join(", ") : "no addresses";
  return `${i.name} (${addrs})${i.up ? "" : " · down"}`;
}

// Picks an interface from what the node reported, a custom name, or none,
// plus an optional source address.
function BindingEditor({
  title,
  hint,
  value,
  onChange,
  interfaces,
  onRefresh,
  refreshing,
}: {
  title: string;
  hint: string;
  value: Binding;
  onChange: (b: Binding) => void;
  interfaces: NetInterface[];
  onRefresh?: () => void;
  refreshing: boolean;
}) {
  const known = interfaces.some((i) => i.name === value.interface);
  const [custom, setCustom] = useState(value.interface !== "" && !known);
  const listId = useId();
  const choice = custom ? CUSTOM : value.interface;
  const addrs = interfaces.find((i) => i.name === value.interface)?.addrs ?? interfaces.flatMap((i) => i.addrs);

  return (
    <fieldset className="rounded-xl border border-ink-600/60 p-4">
      <legend className="px-1 text-xs font-medium text-slate-300">{title}</legend>
      <p className="mb-3 text-[11px] leading-relaxed text-slate-500">{hint}</p>
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <Field label="Interface">
          {(id) => (
            <div className="space-y-2">
              <div className="flex gap-1">
                <Select
                  id={id}
                  className="min-w-0 flex-1"
                  value={choice}
                  onChange={(v) => {
                    setCustom(v === CUSTOM);
                    if (v !== CUSTOM) onChange({ ...value, interface: v });
                  }}
                  options={[
                    { value: NONE, label: "None (system default)" },
                    ...interfaces.map((i) => ({ value: i.name, label: interfaceLabel(i) })),
                    { value: CUSTOM, label: "Custom name…" },
                  ]}
                />
                {onRefresh && (
                  <IconButton label="Ask the node for its interfaces" busy={refreshing} onClick={onRefresh}>
                    <RefreshCw className="h-4 w-4" />
                  </IconButton>
                )}
              </div>
              {custom && (
                <input
                  className="input font-mono"
                  aria-label={`${title} interface name`}
                  value={value.interface}
                  placeholder="wg0"
                  spellCheck={false}
                  onChange={(e) => onChange({ ...value, interface: e.target.value.trim() })}
                />
              )}
            </div>
          )}
        </Field>
        <Field label="Source IP" hint="Empty lets the system choose.">
          {(id) => (
            <>
              <input
                id={id}
                className="input font-mono"
                list={listId}
                value={value.source_ip}
                placeholder="auto"
                spellCheck={false}
                onChange={(e) => onChange({ ...value, source_ip: e.target.value.trim() })}
              />
              <datalist id={listId}>
                {addrs.map((a) => (
                  <option key={a} value={a} />
                ))}
              </datalist>
            </>
          )}
        </Field>
      </div>
    </fieldset>
  );
}

// The edit form sends only what the operator changed. Name and enabled live in
// the core and can only be saved while it answers; the tunnel settings reach
// the node over its agent connection, so they must not fail along with them.
export function changedFields(node: Node, form: Required<NodePatch>): NodePatch {
  const c = node.config;
  const sameBinding = (a: Binding, b: Binding) => a.interface === b.interface && a.source_ip === b.source_ip;
  const patch: NodePatch = {};
  if (form.name !== node.name) patch.name = form.name;
  if (form.enabled !== node.enabled) patch.enabled = form.enabled;
  if (form.pool_size !== c.pool_size) patch.pool_size = form.pool_size;
  if (form.max_extra !== c.max_extra) patch.max_extra = form.max_extra;
  if (form.connect_timeout_secs !== c.connect_timeout_secs) patch.connect_timeout_secs = form.connect_timeout_secs;
  if (form.allow_local !== c.allow_local) patch.allow_local = form.allow_local;
  if (!sameBinding(form.egress, c.egress)) patch.egress = form.egress;
  if (!sameBinding(form.uplink, c.uplink)) patch.uplink = form.uplink;
  return patch;
}

export function EditNodeModal({
  node,
  onClose,
  onSaved,
}: {
  node: Node;
  onClose: () => void;
  onSaved: (n: Node) => void;
}) {
  const toast = useToast();
  const c = node.config;
  const [name, setName] = useState(node.name);
  const [enabled, setEnabled] = useState(node.enabled);
  const [poolSize, setPoolSize] = useState(String(c.pool_size));
  const [maxExtra, setMaxExtra] = useState(String(c.max_extra));
  const [timeout, setTimeoutSecs] = useState(String(c.connect_timeout_secs));
  const [allowLocal, setAllowLocal] = useState(c.allow_local);
  const [egress, setEgress] = useState<Binding>(c.egress);
  const [uplink, setUplink] = useState<Binding>(c.uplink);
  const [interfaces, setInterfaces] = useState<NetInterface[]>(node.agent?.interfaces ?? []);
  const [refreshing, setRefreshing] = useState(false);
  const [busy, setBusy] = useState(false);

  const nameErr = name.trim() === "" && node.name !== "" ? "A name can be changed but not removed" : undefined;
  const poolErr = intError(poolSize, 1, 4096);
  const extraErr = intError(maxExtra, 0, 65536);
  const timeoutErr = intError(timeout, 1, 600);
  const connected = !!node.agent?.connected;
  const patch = changedFields(node, {
    name: name.trim(),
    enabled,
    pool_size: Number(poolSize),
    max_extra: Number(maxExtra),
    connect_timeout_secs: Number(timeout),
    allow_local: allowLocal,
    egress,
    uplink,
  });
  const dirty = Object.keys(patch).length > 0;

  const refreshInterfaces = async () => {
    setRefreshing(true);
    try {
      setInterfaces((await api.nodeInterfaces(node.id)).interfaces);
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setRefreshing(false);
    }
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      onSaved(await api.updateNode(node.id, patch));
    } catch (e) {
      toast("error", errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`Edit ${node.id}`}
      size="xl"
      onClose={onClose}
      footer={
        <>
          <span className="mr-auto text-xs text-slate-500">
            {connected
              ? "Changes are pushed to the node right away."
              : "The node gets the changes when it next connects."}
          </span>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            type="submit"
            form="edit-node"
            className="btn-primary"
            disabled={busy || !dirty || !!nameErr || !!poolErr || !!extraErr || !!timeoutErr}
          >
            {busy && <Spinner />} Save
          </button>
        </>
      }
    >
      <form id="edit-node" onSubmit={submit} className="space-y-5">
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <Field label="Name" error={nameErr}>
            {(id) => <input id={id} className="input" value={name} onChange={(e) => setName(e.target.value)} />}
          </Field>
          <div className="flex items-end gap-6 pb-2">
            <Toggle checked={enabled} onChange={setEnabled} label="Enabled" />
            <Toggle checked={allowLocal} onChange={setAllowLocal} label="Allow local targets" />
          </div>
        </div>
        <p className="-mt-2 text-[11px] leading-relaxed text-slate-500">
          A disabled node keeps its agent connection but runs no tunnel. Local targets are the node itself (loopback and
          its own addresses) and link-local addresses such as the cloud metadata service at 169.254.169.254; leave them
          blocked unless users need them. Hosts on the node's private LAN are reachable either way.
        </p>

        <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
          <Field label="Pool size" error={poolErr} hint="Idle tunnel streams kept ready.">
            {(id) => (
              <input
                id={id}
                className="input"
                inputMode="numeric"
                value={poolSize}
                onChange={(e) => setPoolSize(e.target.value.trim())}
              />
            )}
          </Field>
          <Field label="Max extra" error={extraErr} hint="Streams opened beyond the pool under load.">
            {(id) => (
              <input
                id={id}
                className="input"
                inputMode="numeric"
                value={maxExtra}
                onChange={(e) => setMaxExtra(e.target.value.trim())}
              />
            )}
          </Field>
          <Field label="Connect timeout (s)" error={timeoutErr} hint="Time to reach a target.">
            {(id) => (
              <input
                id={id}
                className="input"
                inputMode="numeric"
                value={timeout}
                onChange={(e) => setTimeoutSecs(e.target.value.trim())}
              />
            )}
          </Field>
        </div>

        {!node.agent && (
          <p className="text-xs text-amber-300/80">
            The node has not connected yet, so its interfaces are unknown; type a custom name if you need one.
          </p>
        )}
        <BindingEditor
          title="Egress"
          hint="Where the node sends traffic to SOCKS targets."
          value={egress}
          onChange={setEgress}
          interfaces={interfaces}
          onRefresh={connected ? refreshInterfaces : undefined}
          refreshing={refreshing}
        />
        <BindingEditor
          title="Uplink"
          hint="Where the node's tunnel to the WireFox server goes out."
          value={uplink}
          onChange={setUplink}
          interfaces={interfaces}
          onRefresh={connected ? refreshInterfaces : undefined}
          refreshing={refreshing}
        />
      </form>
    </Modal>
  );
}

const LOG_LINES = ["100", "200", "500", "1000"] as const;
type LogLines = (typeof LOG_LINES)[number];

export function NodeLogsModal({ node, onClose }: { node: Node; onClose: () => void }) {
  const [lines, setLines] = useState<LogLines>("200");
  const load = useCallback(() => api.nodeLogs(node.id, Number(lines)), [node.id, lines]);
  const logs = useResource(load);
  const entries = logs.data?.lines ?? [];

  return (
    <Modal title={`Logs of ${node.id}`} size="xl" onClose={onClose}>
      <div className="mb-3 flex items-center justify-between gap-3">
        <span className="text-xs text-slate-500">Fetched from the node over its agent connection.</span>
        <div className="flex items-center gap-2">
          <Select<LogLines>
            aria-label="Lines"
            value={lines}
            onChange={setLines}
            options={LOG_LINES.map((l) => ({ value: l, label: `${l} lines` }))}
          />
          <button className="btn-ghost" onClick={logs.reload} disabled={logs.loading}>
            {logs.loading ? <Spinner /> : <RefreshCw className="h-4 w-4" />} Refresh
          </button>
        </div>
      </div>
      <div className="overflow-hidden rounded-xl border border-ink-600/60">
        {logs.error ? (
          <ErrorState message={logs.error} onRetry={logs.reload} />
        ) : !logs.data ? (
          <LoadingState label="Asking the node…" />
        ) : entries.length === 0 ? (
          <p className="py-12 text-center text-sm text-slate-500">The node has no log lines yet.</p>
        ) : (
          <div className="max-h-[60vh] overflow-y-auto bg-ink-950/60 p-4 font-mono text-xs leading-relaxed">
            {entries.map((l, i) => (
              <div key={i} className="flex gap-3 whitespace-pre-wrap break-all">
                <span className="shrink-0 text-slate-600">{fmtClock(l.unix_ms)}</span>
                <span className={cls("w-12 shrink-0 uppercase", logLevelClass(l.level))}>{l.level}</span>
                <span className="text-slate-300">{l.message}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
}
