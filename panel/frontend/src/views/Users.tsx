import { useState, type FormEvent } from "react";
import { Dices, Link2, Pencil, Plus, RotateCcw, Trash2, UserRound, X } from "lucide-react";
import { api, errorMessage } from "../api";
import { useResource } from "../hooks";
import { generatePassword, portOf, socksUrl } from "../socks";
import { BALANCES, type Balance, type Node, type User, type UserCreate } from "../types";
import {
  Badge,
  ConfirmDialog,
  CopyButton,
  EmptyState,
  ErrorState,
  Field,
  fmtBytes,
  IconButton,
  LoadingState,
  Modal,
  PageHeader,
  Select,
  Spinner,
  TableCard,
  Toggle,
  useToast,
} from "../ui";

type Endpoint = { host: string; port: string | null };

export function Users() {
  const toast = useToast();
  const users = useResource(api.users, 10000);
  const nodes = useResource(api.nodes);
  const settings = useResource(api.settings);
  const core = useResource(api.core);
  const [editing, setEditing] = useState<User | "new" | null>(null);
  const [created, setCreated] = useState<{ user: User; password: string } | null>(null);
  const [deleting, setDeleting] = useState<User | null>(null);
  const [resetting, setResetting] = useState<string | null>(null);

  const endpoint: Endpoint = {
    host: settings.data?.socks_public_host || window.location.hostname,
    port: portOf(core.data?.config.socks_listen[0]),
  };

  const resetTraffic = async (u: User) => {
    setResetting(u.username);
    try {
      const fresh = await api.resetUserTraffic(u.username);
      if (users.data) users.set(users.data.map((x) => (x.username === fresh.username ? fresh : x)));
      toast("success", `Traffic counters of ${u.username} reset`);
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setResetting(null);
    }
  };

  const remove = async (u: User) => {
    try {
      await api.deleteUser(u.username);
      if (users.data) users.set(users.data.filter((x) => x.username !== u.username));
      toast("success", `User ${u.username} deleted`);
    } catch (e) {
      toast("error", errorMessage(e));
      throw e;
    }
  };

  const list = users.data ?? [];

  return (
    <div>
      <PageHeader
        title="Users"
        subtitle="SOCKS accounts, the nodes they may use and how their connections are balanced"
        actions={
          <button className="btn-primary" onClick={() => setEditing("new")}>
            <Plus className="h-4 w-4" /> Add user
          </button>
        }
      />

      {!users.data ? (
        <div className="card">
          {users.error ? <ErrorState message={users.error} onRetry={users.reload} /> : <LoadingState />}
        </div>
      ) : list.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<UserRound className="h-10 w-10" />}
            title="No users yet"
            hint="SOCKS clients authenticate with a username and password. Add the first account to start using the proxy."
            action={
              <button className="btn-primary" onClick={() => setEditing("new")}>
                <Plus className="h-4 w-4" /> Add user
              </button>
            }
          />
        </div>
      ) : (
        <TableCard head={["User", "Nodes", "Balance", "Max conns", "Active", "Down", "Up", ""]}>
          {list.map((u) => (
            <tr key={u.username} className="hover:bg-ink-700/30">
              <td className="td">
                <div className="flex items-center gap-2">
                  <span className={u.enabled ? "font-medium text-slate-100" : "text-slate-500 line-through"}>
                    {u.username}
                  </span>
                  {!u.enabled && <Badge tone="red">disabled</Badge>}
                </div>
                {u.note && <div className="mt-0.5 max-w-[16rem] truncate text-xs text-slate-500">{u.note}</div>}
              </td>
              <td className="td">
                {u.nodes.length === 0 ? (
                  <span className="text-xs text-slate-500">any node</span>
                ) : (
                  <div className="flex max-w-[14rem] flex-wrap gap-1">
                    {u.nodes.map((n) => (
                      <Badge key={n}>{n}</Badge>
                    ))}
                  </div>
                )}
              </td>
              <td className="td whitespace-nowrap">
                {u.balance ? (
                  <Badge tone="fox">{u.balance}</Badge>
                ) : (
                  <span className="text-xs text-slate-500">default</span>
                )}
              </td>
              <td className="td text-slate-300">{u.max_connections ?? <span className="text-slate-500">∞</span>}</td>
              <td className="td text-slate-300">{u.traffic.active_connections}</td>
              <td className="td whitespace-nowrap text-slate-300">{fmtBytes(u.traffic.bytes_down)}</td>
              <td className="td whitespace-nowrap text-slate-300">{fmtBytes(u.traffic.bytes_up)}</td>
              <td className="td">
                <div className="flex justify-end gap-0.5">
                  <CopyButton
                    compact
                    label="Copy socks5:// address (without password)"
                    text={socksUrl(u.username, null, endpoint.host, endpoint.port)}
                  />
                  <IconButton label="Edit" onClick={() => setEditing(u)}>
                    <Pencil className="h-4 w-4" />
                  </IconButton>
                  <IconButton label="Reset traffic" busy={resetting === u.username} onClick={() => resetTraffic(u)}>
                    <RotateCcw className="h-4 w-4" />
                  </IconButton>
                  <IconButton label="Delete" danger onClick={() => setDeleting(u)}>
                    <Trash2 className="h-4 w-4" />
                  </IconButton>
                </div>
              </td>
            </tr>
          ))}
        </TableCard>
      )}

      {editing && (
        <UserModal
          user={editing === "new" ? null : editing}
          nodes={nodes.data ?? []}
          onClose={() => setEditing(null)}
          onSaved={(u, password) => {
            setEditing(null);
            void users.reload();
            if (password) setCreated({ user: u, password });
            toast("success", editing === "new" ? `User ${u.username} created` : `User ${u.username} saved`);
          }}
        />
      )}
      {created && (
        <CreatedModal
          url={socksUrl(created.user.username, created.password, endpoint.host, endpoint.port)}
          user={created.user}
          password={created.password}
          onClose={() => setCreated(null)}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title="Delete user"
          danger
          confirmLabel="Delete"
          message={
            <>
              Delete <span className="font-medium text-slate-100">{deleting.username}</span>? Its open connections are
              closed and clients using it can no longer sign in.
            </>
          }
          onConfirm={() => remove(deleting)}
          onClose={() => setDeleting(null)}
        />
      )}
    </div>
  );
}

function UserModal({
  user,
  nodes,
  onClose,
  onSaved,
}: {
  user: User | null;
  nodes: Node[];
  onClose: () => void;
  // `password` is set when the operator should be shown the new credentials.
  onSaved: (u: User, password: string | null) => void;
}) {
  const toast = useToast();
  const [username, setUsername] = useState(user?.username ?? "");
  const [password, setPassword] = useState("");
  const [enabled, setEnabled] = useState(user?.enabled ?? true);
  const [selected, setSelected] = useState<string[]>(user?.nodes ?? []);
  const [maxConns, setMaxConns] = useState(user?.max_connections?.toString() ?? "");
  const [note, setNote] = useState(user?.note ?? "");
  const [balance, setBalance] = useState<Balance | "">(user?.balance ?? "");
  const [busy, setBusy] = useState(false);

  const maxInvalid = maxConns !== "" && !/^[1-9]\d*$/.test(maxConns);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const fields = {
      enabled,
      nodes: selected,
      max_connections: maxConns === "" ? null : Number(maxConns),
      note: note.trim() === "" ? null : note.trim(),
      balance: balance === "" ? null : balance,
    };
    setBusy(true);
    try {
      if (user) {
        const saved = await api.updateUser(user.username, password ? { ...fields, password } : fields);
        onSaved(saved, password || null);
      } else {
        const body: UserCreate = { username: username.trim(), ...fields };
        if (password) body.password = password;
        const res = await api.createUser(body);
        onSaved(res.user, res.password ?? (password || null));
      }
    } catch (e) {
      toast("error", errorMessage(e));
      setBusy(false);
    }
  };

  const available = nodes.filter((n) => !selected.includes(n.id));

  return (
    <Modal
      title={user ? `Edit ${user.username}` : "Add user"}
      size="lg"
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn-ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            type="submit"
            form="user-form"
            className="btn-primary"
            disabled={busy || maxInvalid || (!user && !username.trim())}
          >
            {busy && <Spinner />}
            {user ? "Save" : "Create"}
          </button>
        </>
      }
    >
      <form id="user-form" onSubmit={submit} className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <Field label="Username">
          {(id) => (
            <input
              id={id}
              className="input"
              value={username}
              disabled={!!user}
              autoComplete="off"
              spellCheck={false}
              onChange={(e) => setUsername(e.target.value)}
            />
          )}
        </Field>
        <Field
          label="Password"
          hint={user ? "Leave empty to keep the current password." : "Leave empty to have one generated."}
        >
          {(id) => (
            <div className="flex gap-2">
              <input
                id={id}
                className="input font-mono"
                value={password}
                autoComplete="new-password"
                spellCheck={false}
                placeholder={user ? "unchanged" : "generated"}
                onChange={(e) => setPassword(e.target.value)}
              />
              <button
                type="button"
                className="btn-ghost shrink-0 px-2.5"
                title="Generate a password"
                aria-label="Generate a password"
                onClick={() => setPassword(generatePassword())}
              >
                <Dices className="h-4 w-4" />
              </button>
            </div>
          )}
        </Field>

        <Field
          label="Nodes"
          className="sm:col-span-2"
          hint="Connections of this user only go through these nodes. Leave empty to allow any node."
        >
          {(id) => (
            <div className="space-y-2">
              <div className="flex min-h-[2.5rem] flex-wrap items-center gap-1.5 rounded-lg border border-ink-500/70 bg-ink-900/70 px-2 py-1.5">
                {selected.length === 0 && <span className="px-1 text-sm text-slate-500">any node</span>}
                {selected.map((n) => (
                  <span
                    key={n}
                    className="inline-flex items-center gap-1 rounded-md border border-fox-500/30 bg-fox-500/15 py-0.5 pl-2 pr-1 text-xs text-fox-300"
                  >
                    {n}
                    <button
                      type="button"
                      className="rounded p-0.5 hover:bg-fox-500/20"
                      aria-label={`Remove ${n}`}
                      onClick={() => setSelected(selected.filter((x) => x !== n))}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </span>
                ))}
              </div>
              {available.length > 0 && (
                <Select
                  id={id}
                  value=""
                  onChange={(v) => v && setSelected([...selected, v])}
                  options={[
                    { value: "", label: "Add a node…" },
                    ...available.map((n) => ({ value: n.id, label: n.name ? `${n.id} (${n.name})` : n.id })),
                  ]}
                />
              )}
            </div>
          )}
        </Field>

        <Field label="Load balancing">
          {(id) => (
            <Select<Balance | "">
              id={id}
              value={balance}
              onChange={setBalance}
              options={[
                { value: "", label: "Server default" },
                ...BALANCES.map((b) => ({ value: b.value, label: `${b.label} (${b.hint})` })),
              ]}
            />
          )}
        </Field>
        <Field
          label="Max connections"
          hint="Concurrent SOCKS connections; empty for no limit."
          error={maxInvalid ? "A positive whole number" : undefined}
        >
          {(id) => (
            <input
              id={id}
              className="input"
              inputMode="numeric"
              value={maxConns}
              placeholder="unlimited"
              onChange={(e) => setMaxConns(e.target.value.trim())}
            />
          )}
        </Field>
        <Field label="Note" className="sm:col-span-2">
          {(id) => (
            <input
              id={id}
              className="input"
              value={note}
              placeholder="Who or what this account is for"
              onChange={(e) => setNote(e.target.value)}
            />
          )}
        </Field>
        <div className="sm:col-span-2">
          <Toggle checked={enabled} onChange={setEnabled} label="Enabled" />
        </div>
      </form>
    </Modal>
  );
}

function CreatedModal({
  user,
  password,
  url,
  onClose,
}: {
  user: User;
  password: string;
  url: string;
  onClose: () => void;
}) {
  return (
    <Modal
      title={`Credentials for ${user.username}`}
      onClose={onClose}
      footer={
        <button className="btn-primary" onClick={onClose}>
          Done
        </button>
      }
    >
      <div className="space-y-4 text-sm">
        <p className="text-slate-400">
          The password is shown only now. Copy it or the full address before closing this window.
        </p>
        <div>
          <div className="label">Password</div>
          <div className="flex items-center gap-2 rounded-lg border border-ink-500/70 bg-ink-900/70 py-1 pl-3 pr-1">
            <code className="min-w-0 flex-1 break-all font-mono text-slate-100">{password}</code>
            <CopyButton compact text={password} label="Copy password" />
          </div>
        </div>
        <div>
          <div className="label flex items-center gap-1.5">
            <Link2 className="h-3.5 w-3.5" /> SOCKS address
          </div>
          <div className="flex items-center gap-2 rounded-lg border border-ink-500/70 bg-ink-900/70 py-1 pl-3 pr-1">
            <code className="min-w-0 flex-1 break-all font-mono text-fox-300">{url}</code>
            <CopyButton compact text={url} label="Copy address" />
          </div>
        </div>
      </div>
    </Modal>
  );
}
