import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  Cable,
  ChevronUp,
  Cpu,
  KeyRound,
  LayoutDashboard,
  LogOut,
  Server,
  Settings as SettingsIcon,
  Users as UsersIcon,
  type LucideIcon,
} from "lucide-react";
import { api, setUnauthorizedHandler } from "./api";
import { useResource } from "./hooks";
import { coreState } from "./status";
import type { Overview } from "./types";
import { cls, Logo, StatusDot, useToast } from "./ui";
import { ActivityView } from "./views/Activity";
import { Connections } from "./views/Connections";
import { Core } from "./views/Core";
import { Dashboard } from "./views/Dashboard";
import { Login } from "./views/Login";
import { Nodes } from "./views/Nodes";
import { SettingsView } from "./views/Settings";
import { Users } from "./views/Users";

export type ViewId = "dashboard" | "nodes" | "users" | "connections" | "core" | "settings" | "activity";

const NAV: { id: ViewId; label: string; icon: LucideIcon }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "nodes", label: "Nodes", icon: Server },
  { id: "users", label: "Users", icon: UsersIcon },
  { id: "connections", label: "Connections", icon: Cable },
  { id: "core", label: "Core", icon: Cpu },
  { id: "settings", label: "Settings", icon: SettingsIcon },
  { id: "activity", label: "Activity", icon: Activity },
];

function viewFromHash(): ViewId {
  const id = window.location.hash.replace(/^#\/?/, "");
  return NAV.find((n) => n.id === id)?.id ?? "dashboard";
}

export default function App() {
  // undefined while the session check is in flight, null when signed out.
  const [user, setUser] = useState<string | null | undefined>(undefined);

  useEffect(() => {
    setUnauthorizedHandler(() => setUser(null));
    api
      .me()
      .then((m) => setUser(m.username))
      .catch(() => setUser(null));
    return () => setUnauthorizedHandler(null);
  }, []);

  if (user === undefined) {
    return (
      <div className="grid h-full place-items-center">
        <Logo className="h-10 w-10 animate-pulse" />
      </div>
    );
  }
  if (user === null) return <Login onSuccess={setUser} />;
  return <Shell user={user} onSignedOut={() => setUser(null)} />;
}

function Shell({ user, onSignedOut }: { user: string; onSignedOut: () => void }) {
  const [view, setView] = useState<ViewId>(viewFromHash);
  const overview = useResource<Overview>(api.overview, 5000);

  useEffect(() => {
    const onHash = () => setView(viewFromHash());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  const go = useCallback((id: ViewId) => {
    window.location.hash = `/${id}`;
  }, []);

  return (
    <div className="flex h-full">
      <aside className="flex w-60 shrink-0 flex-col border-r border-ink-600/50 bg-ink-800/40 backdrop-blur-sm">
        <div className="flex items-center gap-2.5 px-5 py-5">
          <Logo className="h-9 w-9 drop-shadow-[0_4px_14px_rgba(249,115,22,0.35)]" />
          <div>
            <div className="font-semibold leading-tight text-slate-100">WireFox</div>
            <div className="text-[11px] leading-tight text-slate-500">Control Panel</div>
          </div>
        </div>

        <nav className="mt-2 flex-1 space-y-1 overflow-y-auto px-3" aria-label="Main">
          {NAV.map((n) => {
            const Icon = n.icon;
            const active = view === n.id;
            return (
              <a
                key={n.id}
                href={`#/${n.id}`}
                aria-current={active ? "page" : undefined}
                className={cls(
                  "flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-sm font-medium transition",
                  active
                    ? "bg-fox-500/15 text-fox-300 shadow-[inset_0_0_0_1px_rgba(249,115,22,0.3)]"
                    : "text-slate-400 hover:bg-ink-600/40 hover:text-slate-100",
                )}
              >
                <Icon className="h-[18px] w-[18px]" />
                {n.label}
              </a>
            );
          })}
        </nav>

        <SidebarStatus overview={overview.data} failed={!!overview.error} />
        <UserMenu user={user} onSettings={() => go("settings")} onSignedOut={onSignedOut} />
      </aside>

      <main className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-7xl px-6 py-6">
          {view === "dashboard" && <Dashboard overview={overview} go={go} />}
          {view === "nodes" && <Nodes />}
          {view === "users" && <Users />}
          {view === "connections" && <Connections />}
          {view === "core" && <Core />}
          {view === "settings" && <SettingsView />}
          {view === "activity" && <ActivityView />}
        </div>
      </main>
    </div>
  );
}

function SidebarStatus({ overview, failed }: { overview: Overview | undefined; failed: boolean }) {
  if (!overview) {
    return (
      <div className="card m-3 p-4 text-xs text-slate-500">{failed ? "Panel API unreachable" : "Loading status…"}</div>
    );
  }
  const core = coreState(overview.unit, overview.core?.reachable ?? false);
  return (
    <div className="card m-3 space-y-2 p-4 text-xs">
      <div className="flex items-center justify-between">
        <span className="text-slate-400">Nodes online</span>
        <span className="font-medium text-slate-200">
          {overview.core ? `${overview.core.nodes_online}/${overview.core.nodes_total}` : "—"}
        </span>
      </div>
      <div className="flex items-center justify-between">
        <span className="text-slate-400">Agents connected</span>
        <span className="font-medium text-slate-200">
          {overview.agents.connected}/{overview.agents.total}
        </span>
      </div>
      <div className="flex items-center gap-2 pt-1 text-slate-400">
        <StatusDot tone={core.tone} pulse={core.tone === "green"} />
        <span className="truncate" title={core.label}>
          Core {core.label}
        </span>
      </div>
      {failed && <p className="text-amber-300/80">Status may be stale</p>}
    </div>
  );
}

function UserMenu({
  user,
  onSettings,
  onSignedOut,
}: {
  user: string;
  onSettings: () => void;
  onSignedOut: () => void;
}) {
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const logout = async () => {
    try {
      await api.logout();
    } catch {
      toast("info", "Signed out locally; the panel did not confirm");
    }
    onSignedOut();
  };

  return (
    <div ref={ref} className="relative px-3 pb-3">
      {open && (
        <div
          className="card absolute bottom-full left-3 right-3 mb-1 animate-fade-in overflow-hidden bg-ink-800/95 p-1 shadow-glow"
          role="menu"
        >
          <button
            role="menuitem"
            className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-sm text-slate-300 hover:bg-ink-600/50"
            onClick={() => {
              setOpen(false);
              onSettings();
            }}
          >
            <KeyRound className="h-4 w-4" /> Change password
          </button>
          <button
            role="menuitem"
            className="flex w-full items-center gap-2 rounded-lg px-3 py-2 text-sm text-rose-300 hover:bg-rose-500/10"
            onClick={logout}
          >
            <LogOut className="h-4 w-4" /> Sign out
          </button>
        </div>
      )}
      <button
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex w-full items-center justify-between gap-2 rounded-xl px-3 py-2.5 text-sm text-slate-400 transition hover:bg-ink-600/40 hover:text-slate-100"
      >
        <span className="flex min-w-0 items-center gap-2">
          <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-fox-500/20 text-[11px] font-semibold text-fox-300">
            {user.charAt(0).toUpperCase() || "?"}
          </span>
          <span className="truncate">{user}</span>
        </span>
        <ChevronUp className={cls("h-4 w-4 shrink-0 transition", !open && "rotate-180")} />
      </button>
    </div>
  );
}
