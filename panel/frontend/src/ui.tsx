import { createContext, useCallback, useContext, useEffect, useId, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, Check, CheckCircle2, ChevronDown, Copy, Info, Loader2, X } from "lucide-react";

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

// Copies to the clipboard. navigator.clipboard only exists on secure origins,
// and the panel is often opened over plain HTTP, so fall back to execCommand.
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to the legacy path
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.top = "-9999px";
  document.body.appendChild(ta);
  try {
    ta.select();
    ta.setSelectionRange(0, text.length);
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    document.body.removeChild(ta);
  }
}

type ToastKind = "success" | "error" | "info";
type Toast = { id: number; kind: ToastKind; msg: string };

const ToastCtx = createContext<(kind: ToastKind, msg: string) => void>(() => {});
export const useToast = () => useContext(ToastCtx);

let toastSeq = 0;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const dismiss = useCallback((id: number) => setToasts((ts) => ts.filter((t) => t.id !== id)), []);
  const push = useCallback(
    (kind: ToastKind, msg: string) => {
      const id = ++toastSeq;
      setToasts((ts) => [...ts.slice(-4), { id, kind, msg }]);
      window.setTimeout(() => dismiss(id), kind === "error" ? 7000 : 4000);
    },
    [dismiss],
  );
  return (
    <ToastCtx.Provider value={push}>
      {children}
      {createPortal(
        <div className="fixed bottom-5 right-5 z-[60] flex w-80 flex-col gap-2" role="status" aria-live="polite">
          {toasts.map((t) => (
            <div
              key={t.id}
              className={cls(
                "card flex animate-fade-in items-start gap-3 px-4 py-3 text-sm shadow-glow",
                t.kind === "error" && "border-rose-500/50",
                t.kind === "success" && "border-emerald-500/40",
              )}
            >
              {t.kind === "success" && <CheckCircle2 className="h-5 w-5 shrink-0 text-emerald-400" />}
              {t.kind === "error" && <AlertTriangle className="h-5 w-5 shrink-0 text-rose-400" />}
              {t.kind === "info" && <Info className="h-5 w-5 shrink-0 text-sky-400" />}
              <span className="min-w-0 flex-1 break-words text-slate-200">{t.msg}</span>
              <button
                onClick={() => dismiss(t.id)}
                className="text-slate-500 hover:text-slate-200"
                aria-label="Dismiss notification"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
          ))}
        </div>,
        document.body,
      )}
    </ToastCtx.Provider>
  );
}

// Open modals, innermost last: Escape only closes the one on top.
const modalStack: string[] = [];

export function Modal({
  onClose,
  title,
  children,
  footer,
  size = "md",
}: {
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  size?: "sm" | "md" | "lg" | "xl";
}) {
  const id = useId();
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const panelRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    modalStack.push(id);
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const first = panelRef.current?.querySelector<HTMLElement>("[autofocus], input, select, textarea");
    (first ?? panelRef.current)?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && modalStack[modalStack.length - 1] === id) {
        e.preventDefault();
        closeRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      modalStack.splice(modalStack.indexOf(id), 1);
      previous?.focus();
    };
  }, [id]);

  // Portalled to <body>: the backdrop blur on .card creates a containing block
  // that would otherwise trap a fixed overlay inside the card it was opened from.
  return createPortal(
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 p-4 backdrop-blur-sm sm:p-8"
      onMouseDown={(e) => e.target === e.currentTarget && onClose()}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${id}-title`}
        tabIndex={-1}
        className={cls(
          "card my-4 w-full animate-fade-in bg-ink-800/95 shadow-glow outline-none",
          size === "sm" && "max-w-md",
          size === "md" && "max-w-lg",
          size === "lg" && "max-w-2xl",
          size === "xl" && "max-w-4xl",
        )}
      >
        <div className="flex items-center justify-between gap-4 border-b border-ink-600/60 px-5 py-4">
          <h3 id={`${id}-title`} className="min-w-0 truncate font-semibold text-slate-100">
            {title}
          </h3>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-100" aria-label="Close">
            <X className="h-5 w-5" />
          </button>
        </div>
        <div className="p-5">{children}</div>
        {footer && (
          <div className="flex items-center justify-end gap-2 border-t border-ink-600/60 px-5 py-3.5">{footer}</div>
        )}
      </div>
    </div>,
    document.body,
  );
}

export function ConfirmDialog({
  title,
  message,
  confirmLabel = "Confirm",
  danger,
  typeToConfirm,
  onConfirm,
  onClose,
}: {
  title: string;
  message: ReactNode;
  confirmLabel?: string;
  danger?: boolean;
  // When set, the operator has to type this text before confirming.
  typeToConfirm?: string;
  onConfirm: () => Promise<unknown> | void;
  onClose: () => void;
}) {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const blocked = typeToConfirm !== undefined && typed !== typeToConfirm;

  const confirm = async () => {
    setBusy(true);
    try {
      await onConfirm();
      onClose();
    } catch {
      // onConfirm reports its own errors; keep the dialog open for a retry.
      setBusy(false);
    }
  };

  return (
    <Modal
      title={title}
      size="sm"
      onClose={onClose}
      footer={
        <>
          <button className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button className={danger ? "btn-danger" : "btn-primary"} disabled={blocked || busy} onClick={confirm}>
            {busy && <Spinner className="h-4 w-4" />}
            {confirmLabel}
          </button>
        </>
      }
    >
      <div className="space-y-4 text-sm text-slate-300">
        <div>{message}</div>
        {typeToConfirm !== undefined && (
          <Field label={`Type ${typeToConfirm} to confirm`}>
            {(id) => (
              <input
                id={id}
                className="input font-mono"
                value={typed}
                autoComplete="off"
                spellCheck={false}
                onChange={(e) => setTyped(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && !blocked && confirm()}
              />
            )}
          </Field>
        )}
      </div>
    </Modal>
  );
}

// Field renders a label bound to its control. Children get the generated id.
export function Field({
  label,
  hint,
  error,
  children,
  className,
}: {
  label: ReactNode;
  hint?: ReactNode;
  error?: string;
  children: (id: string) => ReactNode;
  className?: string;
}) {
  const id = useId();
  return (
    <div className={className}>
      <label htmlFor={id} className="label">
        {label}
      </label>
      {children(id)}
      {error ? (
        <p className="mt-1 text-[11px] text-rose-400">{error}</p>
      ) : (
        hint && <p className="mt-1 text-[11px] leading-relaxed text-slate-500">{hint}</p>
      )}
    </div>
  );
}

export function Toggle({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className="group inline-flex items-center gap-2.5 rounded-full focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40"
    >
      <span
        className={cls(
          "relative h-5 w-9 shrink-0 rounded-full transition group-focus-visible:ring-2 group-focus-visible:ring-fox-500/40",
          checked ? "bg-fox-500" : "bg-ink-500",
        )}
      >
        <span
          className={cls(
            "absolute left-0.5 top-0.5 h-4 w-4 rounded-full bg-white transition-transform",
            checked && "translate-x-4",
          )}
        />
      </span>
      <span className="text-sm text-slate-300">{label}</span>
    </button>
  );
}

export function Select<T extends string>({
  id,
  value,
  onChange,
  options,
  disabled,
  className,
  "aria-label": ariaLabel,
}: {
  id?: string;
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
}) {
  return (
    <div className={cls("relative", className)}>
      <select
        id={id}
        aria-label={ariaLabel}
        className="input appearance-none pr-8"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value as T)}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </select>
      <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-slate-500" />
    </div>
  );
}

export type Tone = "slate" | "green" | "red" | "amber" | "fox" | "sky" | "violet";

const DOT_TONES: Record<Tone, string> = {
  slate: "bg-slate-500",
  green: "bg-emerald-400",
  red: "bg-rose-400",
  amber: "bg-amber-400",
  fox: "bg-fox-400",
  sky: "bg-sky-400",
  violet: "bg-violet-400",
};

export function StatusDot({ tone, pulse, label }: { tone: Tone; pulse?: boolean; label?: string }) {
  return (
    <span
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={cls("inline-block h-2 w-2 shrink-0 rounded-full", DOT_TONES[tone], pulse && "animate-pulse-dot")}
    />
  );
}

const BADGE_TONES: Record<Tone, string> = {
  slate: "bg-ink-600/70 text-slate-300 border-ink-500/60",
  green: "bg-emerald-500/15 text-emerald-300 border-emerald-500/30",
  red: "bg-rose-500/15 text-rose-300 border-rose-500/30",
  amber: "bg-amber-500/15 text-amber-300 border-amber-500/30",
  fox: "bg-fox-500/15 text-fox-300 border-fox-500/30",
  sky: "bg-sky-500/15 text-sky-300 border-sky-500/30",
  violet: "bg-violet-500/15 text-violet-300 border-violet-500/30",
};

export function Badge({ children, tone = "slate", title }: { children: ReactNode; tone?: Tone; title?: string }) {
  return (
    <span
      title={title}
      className={cls(
        "inline-flex items-center gap-1 whitespace-nowrap rounded-md border px-2 py-0.5 text-[11px] font-medium",
        BADGE_TONES[tone],
      )}
    >
      {children}
    </span>
  );
}

export function Spinner({ className }: { className?: string }) {
  return <Loader2 aria-hidden className={cls("animate-spin", className ?? "h-4 w-4")} />;
}

export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon: ReactNode;
  title: string;
  hint?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center px-6 py-14 text-center">
      <div className="mb-3 text-slate-600">{icon}</div>
      <p className="font-medium text-slate-300">{title}</p>
      {hint && <p className="mt-1 max-w-md text-sm text-slate-500">{hint}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

export function LoadingState({ label = "Loading…" }: { label?: string }) {
  return (
    <div className="flex items-center justify-center gap-2 py-14 text-sm text-slate-500">
      <Spinner /> {label}
    </div>
  );
}

export function ErrorState({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="flex flex-col items-center justify-center gap-3 px-6 py-12 text-center">
      <AlertTriangle className="h-8 w-8 text-rose-400/80" />
      <p className="max-w-md text-sm text-slate-400">{message}</p>
      {onRetry && (
        <button className="btn-ghost btn-sm" onClick={onRetry}>
          Try again
        </button>
      )}
    </div>
  );
}

export function Banner({
  tone,
  icon,
  children,
}: {
  tone: "amber" | "red" | "sky";
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      className={cls(
        "flex items-start gap-3 rounded-xl border px-4 py-3 text-sm",
        tone === "amber" && "border-amber-500/30 bg-amber-500/10 text-amber-200",
        tone === "red" && "border-rose-500/30 bg-rose-500/10 text-rose-200",
        tone === "sky" && "border-sky-500/30 bg-sky-500/10 text-sky-200",
      )}
    >
      {icon && <span className="mt-0.5 shrink-0">{icon}</span>}
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

export function IconButton({
  label,
  onClick,
  busy,
  disabled,
  danger,
  children,
}: {
  label: string;
  onClick: () => void;
  busy?: boolean;
  disabled?: boolean;
  danger?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      onClick={onClick}
      disabled={busy || disabled}
      className={cls(
        "grid h-8 w-8 place-items-center rounded-lg transition disabled:opacity-40",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-fox-500/40",
        danger
          ? "text-rose-400/80 hover:bg-rose-500/10 hover:text-rose-300"
          : "text-slate-400 hover:bg-ink-600/60 hover:text-slate-100",
      )}
    >
      {busy ? <Spinner /> : children}
    </button>
  );
}

export function CopyButton({ text, label = "Copy", compact }: { text: string; label?: string; compact?: boolean }) {
  const toast = useToast();
  const [done, setDone] = useState(false);
  useEffect(() => {
    if (!done) return;
    const t = window.setTimeout(() => setDone(false), 1500);
    return () => window.clearTimeout(t);
  }, [done]);

  const copy = async () => {
    if (await copyText(text)) setDone(true);
    else toast("error", "Copying failed; select the text and copy it by hand");
  };
  const icon = done ? <Check className="h-4 w-4 text-emerald-400" /> : <Copy className="h-4 w-4" />;
  return compact ? (
    <IconButton label={label} onClick={copy}>
      {icon}
    </IconButton>
  ) : (
    <button type="button" className="btn-ghost btn-sm" onClick={copy}>
      {icon}
      {done ? "Copied" : label}
    </button>
  );
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex flex-wrap items-end justify-between gap-4">
      <div className="min-w-0">
        <h1 className="text-2xl font-semibold text-slate-100">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-slate-500">{subtitle}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

export function SectionTitle({ children, actions }: { children: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-3 flex items-center justify-between gap-3">
      <h2 className="font-semibold text-slate-200">{children}</h2>
      {actions}
    </div>
  );
}

// A card holding a horizontally scrollable table; `head` lists the column titles.
export function TableCard({ head, children }: { head: ReactNode[]; children: ReactNode }) {
  return (
    <div className="card overflow-x-auto">
      <table className="w-full text-sm">
        <thead className="border-b border-ink-600/60">
          <tr>
            {head.map((h, i) => (
              <th key={i} className="th">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-ink-600/40">{children}</tbody>
      </table>
    </div>
  );
}

export function Logo({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 32 32" className={className} aria-hidden>
      <defs>
        <linearGradient id="wfx-logo" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#fb923c" />
          <stop offset="1" stopColor="#ea580c" />
        </linearGradient>
      </defs>
      <rect width="32" height="32" rx="9" fill="url(#wfx-logo)" />
      <path d="M6 7l6 4.5h8L26 7l-1.2 10.5L16 26l-8.8-8.5z" fill="#0b0e15" />
      <path d="M11.5 15.5l2.5 1.5M20.5 15.5L18 17" stroke="#fb923c" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}

export interface MenuItem {
  label: string;
  icon: ReactNode;
  onSelect: () => void;
  disabled?: boolean;
  danger?: boolean;
  // Shown as a tooltip when the item is disabled.
  hint?: string;
}

// A dropdown of row actions. The list is portalled with fixed positioning so
// scrolling table containers don't clip it.
export function Menu({ label, icon, items }: { label: string; icon: ReactNode; items: MenuItem[] }) {
  const [pos, setPos] = useState<{ top: number; right: number } | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!pos) return;
    const close = () => setPos(null);
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!listRef.current?.contains(t) && !buttonRef.current?.contains(t)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        close();
        buttonRef.current?.focus();
      }
    };
    listRef.current?.querySelector<HTMLElement>("button:not(:disabled)")?.focus();
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [pos]);

  const toggle = () => {
    const r = buttonRef.current?.getBoundingClientRect();
    if (pos || !r) setPos(null);
    else setPos({ top: r.bottom + 4, right: window.innerWidth - r.right });
  };

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={!!pos}
        onClick={toggle}
        className="grid h-8 w-8 place-items-center rounded-lg text-slate-400 transition hover:bg-ink-600/60 hover:text-slate-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-fox-500/40"
      >
        {icon}
      </button>
      {pos &&
        createPortal(
          <div
            ref={listRef}
            role="menu"
            style={{ top: pos.top, right: pos.right }}
            className="card fixed z-40 min-w-[13rem] animate-fade-in bg-ink-800/95 p-1 shadow-glow"
          >
            {items.map((it) => (
              <button
                key={it.label}
                role="menuitem"
                type="button"
                disabled={it.disabled}
                title={it.disabled ? it.hint : undefined}
                onClick={() => {
                  setPos(null);
                  it.onSelect();
                }}
                className={cls(
                  "flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm transition",
                  "focus-visible:bg-ink-600/50 focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-40",
                  it.danger ? "text-rose-300 hover:bg-rose-500/10" : "text-slate-300 hover:bg-ink-600/50",
                )}
              >
                {it.icon}
                {it.label}
              </button>
            ))}
          </div>,
          document.body,
        )}
    </>
  );
}
