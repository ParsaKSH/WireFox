import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import {
  AlertTriangle,
  Info,
  Play,
  Plus,
  RefreshCw,
  RotateCw,
  Save,
  ScrollText,
  Square,
  Trash2,
  Undo2,
} from "lucide-react";
import { api, errorMessage } from "../api";
import { useResource } from "../hooks";
import { PRIORITY_NAMES, priorityClass, unitTone } from "../status";
import { BALANCES, type Balance, type CoreConfig, type CoreInfo } from "../types";
import {
  Badge,
  Banner,
  cls,
  ConfirmDialog,
  CopyButton,
  ErrorState,
  Field,
  fmtAgo,
  fmtClock,
  fmtDateTime,
  IconButton,
  LoadingState,
  PageHeader,
  SectionTitle,
  Select,
  Spinner,
  StatusDot,
  Toggle,
  useToast,
} from "../ui";

type Action = "start" | "stop" | "restart";

// Remembered for the browser session so the banner survives switching views
// until the core is actually restarted.
const RESTART_KEY = "wirefox.core.restartRequired";

export function Core() {
  const toast = useToast();
  const core = useResource(api.core, 5000);
  const [restartRequired, setRestartRequired] = useState(() => sessionStorage.getItem(RESTART_KEY) === "1");
  const [confirming, setConfirming] = useState<Action | null>(null);
  const [running, setRunning] = useState<Action | null>(null);

  const markRestartRequired = (v: boolean) => {
    setRestartRequired(v);
    if (v) sessionStorage.setItem(RESTART_KEY, "1");
    else sessionStorage.removeItem(RESTART_KEY);
  };

  const run = async (action: Action) => {
    setRunning(action);
    try {
      await api.coreAction(action);
      if (action !== "stop") markRestartRequired(false);
      toast("success", `Core ${action === "stop" ? "stopped" : action === "start" ? "started" : "restarted"}`);
      void core.reload();
    } catch (e) {
      toast("error", errorMessage(e));
      throw e;
    } finally {
      setRunning(null);
    }
  };

  // Stop and restart drop live connections, so they go through a confirmation.
  const onAction = (action: Action) => {
    if (action === "start") run(action).catch(() => undefined);
    else setConfirming(action);
  };

  const info = core.data;
  const managed = info?.mode === "systemd";

  return (
    <div>
      <PageHeader title="Core" subtitle="The WireFox server: its service, configuration and logs" />

      {!info ? (
        <div className="card">
          {core.error ? <ErrorState message={core.error} onRetry={core.reload} /> : <LoadingState />}
        </div>
      ) : (
        <div className="space-y-6">
          {restartRequired && (
            <Banner tone="amber" icon={<AlertTriangle className="h-4 w-4" />}>
              <div className="flex flex-wrap items-center justify-between gap-3">
                <span>
                  The saved configuration has changes the running core only picks up after a restart.
                  {!managed && " Restart it wherever it is managed."}
                </span>
                <div className="flex gap-2">
                  {managed && (
                    <button className="btn-primary btn-sm" onClick={() => setConfirming("restart")}>
                      <RotateCw className="h-3.5 w-3.5" /> Restart now
                    </button>
                  )}
                  <button className="btn-ghost btn-sm" onClick={() => markRestartRequired(false)}>
                    Dismiss
                  </button>
                </div>
              </div>
            </Banner>
          )}

          <StatusCard info={info} running={running} onAction={onAction} />

          <ConfigCard
            info={info}
            onSaved={(needsRestart) => {
              if (needsRestart) markRestartRequired(true);
              void core.reload();
            }}
          />

          {managed ? (
            <LogsCard />
          ) : (
            <div>
              <SectionTitle>Logs</SectionTitle>
              <div className="card p-5">
                <Banner tone="sky" icon={<Info className="h-4 w-4" />}>
                  The core runs in external mode: the panel writes its configuration but does not run the service, so
                  start, stop, restart and logs are handled wherever the core is managed.
                </Banner>
              </div>
            </div>
          )}
        </div>
      )}

      {confirming && confirming !== "start" && (
        <ConfirmDialog
          title={confirming === "stop" ? "Stop the core" : "Restart the core"}
          danger={confirming === "stop"}
          confirmLabel={confirming === "stop" ? "Stop" : "Restart"}
          message={
            confirming === "stop"
              ? "Stopping the core closes every active SOCKS connection and disconnects all exit-node tunnels until it is started again."
              : "Restarting the core drops every active SOCKS connection; clients and exit-nodes reconnect within a few seconds."
          }
          onConfirm={() => run(confirming)}
          onClose={() => setConfirming(null)}
        />
      )}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 py-2 text-sm">
      <dt className="shrink-0 text-slate-500">{label}</dt>
      <dd className="min-w-0 text-right text-slate-200">{children}</dd>
    </div>
  );
}

function StatusCard({
  info,
  running,
  onAction,
}: {
  info: CoreInfo;
  running: Action | null;
  onAction: (a: Action) => void;
}) {
  const unit = info.unit;
  const active = unit?.active_state === "active";
  return (
    <div className="grid grid-cols-1 gap-6 xl:grid-cols-2">
      <div>
        <SectionTitle>Service</SectionTitle>
        <div className="card p-5">
          <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2.5">
              <StatusDot tone={unit ? unitTone(unit.active_state) : info.reachable ? "green" : "red"} pulse={active} />
              <span className="text-lg font-semibold text-slate-100">
                {unit ? `${unit.active_state}${unit.sub_state ? ` (${unit.sub_state})` : ""}` : "external"}
              </span>
              <Badge tone={info.mode === "systemd" ? "fox" : "slate"}>{info.mode}</Badge>
            </div>
            {unit && (
              <div className="flex gap-2">
                <button className="btn-ghost btn-sm" disabled={!!running || active} onClick={() => onAction("start")}>
                  {running === "start" ? <Spinner className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />} Start
                </button>
                <button className="btn-ghost btn-sm" disabled={!!running} onClick={() => onAction("restart")}>
                  {running === "restart" ? <Spinner className="h-3.5 w-3.5" /> : <RotateCw className="h-3.5 w-3.5" />}{" "}
                  Restart
                </button>
                <button className="btn-ghost btn-sm" disabled={!!running || !active} onClick={() => onAction("stop")}>
                  {running === "stop" ? <Spinner className="h-3.5 w-3.5" /> : <Square className="h-3.5 w-3.5" />} Stop
                </button>
              </div>
            )}
          </div>
          {!unit && (
            <p className="mb-3 text-xs leading-relaxed text-slate-500">
              External mode: the panel writes the core's configuration and talks to its API, but the service itself is
              started and stopped outside the panel.
            </p>
          )}
          <dl className="divide-y divide-ink-600/40">
            {unit && (
              <>
                <Row label="Unit">
                  <span className="font-mono">{unit.name}</span>
                </Row>
                <Row label="Main PID">{unit.main_pid || "—"}</Row>
                <Row label="Since">
                  <span title={fmtDateTime(unit.since)}>{unit.since ? fmtAgo(unit.since) : "—"}</span>
                </Row>
              </>
            )}
            <Row label="API">
              {info.reachable ? <Badge tone="green">reachable</Badge> : <Badge tone="red">not answering</Badge>}
            </Row>
            <Row label="Fingerprint">
              {info.fingerprint ? (
                <span className="flex items-center justify-end gap-1">
                  <span className="break-all font-mono text-xs" title={info.fingerprint}>
                    {info.fingerprint}
                  </span>
                  <CopyButton compact text={info.fingerprint} label="Copy fingerprint" />
                </span>
              ) : (
                <span className="text-slate-500">unknown while the core is unreachable</span>
              )}
            </Row>
          </dl>
        </div>
      </div>
      <div>
        <SectionTitle>Files</SectionTitle>
        <div className="card p-5">
          <dl className="divide-y divide-ink-600/40">
            <Row label="Config">
              <span className="break-all font-mono text-xs">{info.paths.config}</span>
            </Row>
            <Row label="Unit file">
              <span className="break-all font-mono text-xs">{info.paths.unit || "—"}</span>
            </Row>
            <Row label="Binary">
              <span className="break-all font-mono text-xs">{info.paths.binary || "—"}</span>
            </Row>
            <Row label="Data">
              <span className="break-all font-mono text-xs">{info.paths.data || "—"}</span>
            </Row>
          </dl>
        </div>
      </div>
    </div>
  );
}

function ListEditor({
  label,
  hint,
  values,
  onChange,
  placeholder,
}: {
  label: string;
  hint: string;
  values: string[];
  onChange: (v: string[]) => void;
  placeholder: string;
}) {
  return (
    <fieldset>
      <legend className="label">{label}</legend>
      <div className="space-y-2">
        {values.map((v, i) => (
          <div key={i} className="flex gap-2">
            <input
              className="input font-mono"
              aria-label={`${label} ${i + 1}`}
              value={v}
              placeholder={placeholder}
              spellCheck={false}
              onChange={(e) => onChange(values.map((x, j) => (j === i ? e.target.value : x)))}
            />
            <IconButton
              label={`Remove ${label} ${i + 1}`}
              danger
              onClick={() => onChange(values.filter((_, j) => j !== i))}
            >
              <Trash2 className="h-4 w-4" />
            </IconButton>
          </div>
        ))}
        <button type="button" className="btn-ghost btn-sm" onClick={() => onChange([...values, ""])}>
          <Plus className="h-3.5 w-3.5" /> Add
        </button>
      </div>
      <p className="mt-1 text-[11px] leading-relaxed text-slate-500">{hint}</p>
    </fieldset>
  );
}

function timeoutError(v: string): string | undefined {
  if (!/^\d+$/.test(v)) return "A whole number of seconds";
  const n = Number(v);
  return n < 1 || n > 600 ? "Between 1 and 600" : undefined;
}

type ConfigForm = Omit<CoreConfig, "handshake_timeout_secs" | "connect_timeout_secs"> & {
  handshake_timeout_secs: string;
  connect_timeout_secs: string;
};

function toForm(c: CoreConfig): ConfigForm {
  return {
    ...c,
    handshake_timeout_secs: String(c.handshake_timeout_secs),
    connect_timeout_secs: String(c.connect_timeout_secs),
  };
}

function fromForm(f: ConfigForm): CoreConfig {
  const clean = (xs: string[]) => xs.map((x) => x.trim()).filter(Boolean);
  return {
    tunnel_listen: f.tunnel_listen.trim(),
    tunnel_public_addr: f.tunnel_public_addr.trim(),
    socks_listen: clean(f.socks_listen),
    socks_tls_listen: clean(f.socks_tls_listen),
    handshake_timeout_secs: Number(f.handshake_timeout_secs),
    connect_timeout_secs: Number(f.connect_timeout_secs),
    balance: f.balance,
    tls_sans: clean(f.tls_sans),
  };
}

function ConfigCard({ info, onSaved }: { info: CoreInfo; onSaved: (restartRequired: boolean) => void }) {
  const toast = useToast();
  // The form is seeded once; polling must not overwrite what the operator is typing.
  const [form, setForm] = useState<ConfigForm>(() => toForm(info.config));
  const [saving, setSaving] = useState(false);
  const set = <K extends keyof ConfigForm>(k: K, v: ConfigForm[K]) => setForm((f) => ({ ...f, [k]: v }));

  const cleaned = fromForm(form);
  // Both sides go through fromForm so key order and blank list rows don't count as changes.
  const dirty = JSON.stringify(cleaned) !== JSON.stringify(fromForm(toForm(info.config)));
  const hsErr = timeoutError(form.handshake_timeout_secs);
  const ctErr = timeoutError(form.connect_timeout_secs);
  const noSocks = cleaned.socks_listen.length + cleaned.socks_tls_listen.length === 0;
  const invalid = !!hsErr || !!ctErr || !form.tunnel_listen.trim() || noSocks;

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setSaving(true);
    try {
      const res = await api.saveCoreConfig(cleaned);
      setForm(toForm(cleaned));
      toast(
        res.restart_required ? "info" : "success",
        res.restart_required ? "Saved. Restart the core to apply all changes." : "Saved and applied live",
      );
      onSaved(res.restart_required);
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <SectionTitle
        actions={
          <span className="text-xs text-slate-500">
            Balance and timeouts apply live; everything else needs a restart.
          </span>
        }
      >
        Configuration
      </SectionTitle>
      <form onSubmit={save} className="card p-5">
        <div className="grid grid-cols-1 gap-x-6 gap-y-5 lg:grid-cols-2">
          <Field label="Tunnel listen" hint="Address the exit-node tunnels connect to, e.g. 0.0.0.0:7443.">
            {(id) => (
              <input
                id={id}
                className="input font-mono"
                value={form.tunnel_listen}
                spellCheck={false}
                onChange={(e) => set("tunnel_listen", e.target.value)}
              />
            )}
          </Field>
          <Field
            label="Tunnel public address"
            hint="host:port the nodes dial; pushed to every node. Empty derives it from the listen address."
          >
            {(id) => (
              <input
                id={id}
                className="input font-mono"
                value={form.tunnel_public_addr}
                placeholder="relay.example.com:7443"
                spellCheck={false}
                onChange={(e) => set("tunnel_public_addr", e.target.value)}
              />
            )}
          </Field>
          <ListEditor
            label="SOCKS listen"
            hint="Plain SOCKS5 listeners. The first one is used for the addresses shown on the Users page."
            values={form.socks_listen}
            placeholder="0.0.0.0:1080"
            onChange={(v) => set("socks_listen", v)}
          />
          <ListEditor
            label="SOCKS over TLS listen"
            hint="SOCKS5 wrapped in TLS with the core's certificate."
            values={form.socks_tls_listen}
            placeholder="0.0.0.0:1443"
            onChange={(v) => set("socks_tls_listen", v)}
          />
          <Field label="Handshake timeout (s)" error={hsErr} hint="Time a client or node gets to finish its handshake.">
            {(id) => (
              <input
                id={id}
                className="input"
                inputMode="numeric"
                value={form.handshake_timeout_secs}
                onChange={(e) => set("handshake_timeout_secs", e.target.value.trim())}
              />
            )}
          </Field>
          <Field label="Connect timeout (s)" error={ctErr} hint="Time a node gets to reach a SOCKS target.">
            {(id) => (
              <input
                id={id}
                className="input"
                inputMode="numeric"
                value={form.connect_timeout_secs}
                onChange={(e) => set("connect_timeout_secs", e.target.value.trim())}
              />
            )}
          </Field>
          <Field label="Default load balancing" hint="Used for users without their own strategy.">
            {(id) => (
              <Select<Balance>
                id={id}
                value={form.balance}
                onChange={(v) => set("balance", v)}
                options={BALANCES.map((b) => ({ value: b.value, label: `${b.label} (${b.hint})` }))}
              />
            )}
          </Field>
          <ListEditor
            label="TLS names"
            hint="Subject alternative names of the core's self-signed certificate. Changing them issues a new certificate and fingerprint on restart."
            values={form.tls_sans}
            placeholder="relay.example.com"
            onChange={(v) => set("tls_sans", v)}
          />
        </div>
        {noSocks && <p className="mt-4 text-xs text-rose-400">Add at least one SOCKS listener.</p>}
        <div className="mt-5 flex justify-end gap-2 border-t border-ink-600/50 pt-4">
          <button
            type="button"
            className="btn-ghost"
            disabled={!dirty || saving}
            onClick={() => setForm(toForm(info.config))}
          >
            <Undo2 className="h-4 w-4" /> Revert
          </button>
          <button type="submit" className="btn-primary" disabled={!dirty || invalid || saving}>
            {saving ? <Spinner /> : <Save className="h-4 w-4" />} Save
          </button>
        </div>
      </form>
    </div>
  );
}

const LOG_LINES = ["100", "200", "500", "1000"] as const;
type LogLines = (typeof LOG_LINES)[number];

function LogsCard() {
  const [follow, setFollow] = useState(true);
  const [autoScroll, setAutoScroll] = useState(true);
  const [lines, setLines] = useState<LogLines>("200");
  const load = useCallback(() => api.coreLogs(Number(lines)), [lines]);
  const logs = useResource(load, follow ? 5000 : undefined);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = boxRef.current;
    if (el && autoScroll) el.scrollTop = el.scrollHeight;
  }, [logs.data, autoScroll]);

  const entries = logs.data?.entries ?? [];

  return (
    <div>
      <SectionTitle
        actions={
          <div className="flex flex-wrap items-center gap-4">
            <Toggle checked={follow} onChange={setFollow} label="Follow" />
            <Toggle checked={autoScroll} onChange={setAutoScroll} label="Auto-scroll" />
            <Select<LogLines>
              aria-label="Lines"
              value={lines}
              onChange={setLines}
              options={LOG_LINES.map((l) => ({ value: l, label: `${l} lines` }))}
            />
            <IconButton label="Refresh logs" busy={logs.loading} onClick={logs.reload}>
              <RefreshCw className="h-4 w-4" />
            </IconButton>
          </div>
        }
      >
        Logs
      </SectionTitle>
      <div className="card overflow-hidden">
        {!logs.data ? (
          logs.error ? (
            <ErrorState message={logs.error} onRetry={logs.reload} />
          ) : (
            <LoadingState />
          )
        ) : entries.length === 0 ? (
          <div className="flex items-center justify-center gap-2 py-12 text-sm text-slate-500">
            <ScrollText className="h-4 w-4" /> The journal has no entries for the core yet.
          </div>
        ) : (
          <div
            ref={boxRef}
            className="max-h-[28rem] overflow-y-auto bg-ink-950/60 p-4 font-mono text-xs leading-relaxed"
          >
            {entries.map((e, i) => (
              <div key={i} className="flex gap-3 whitespace-pre-wrap break-all">
                <span className="shrink-0 text-slate-600">{fmtClock(e.unix_ms)}</span>
                <span className={cls("w-14 shrink-0", priorityClass(e.priority))}>
                  {PRIORITY_NAMES[e.priority] ?? e.priority}
                </span>
                <span className={priorityClass(e.priority)}>{e.message}</span>
              </div>
            ))}
          </div>
        )}
        {logs.data && logs.error && <p className="px-4 py-2 text-xs text-amber-300/80">Refresh failed: {logs.error}</p>}
      </div>
    </div>
  );
}
