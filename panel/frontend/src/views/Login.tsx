import { useState, type FormEvent } from "react";
import { LogIn } from "lucide-react";
import { api, ApiError, errorMessage } from "../api";
import { Field, Logo, Spinner } from "../ui";

export function Login({ onSuccess }: { onSuccess: (username: string) => void }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setErr("");
    setBusy(true);
    try {
      const me = await api.login(username.trim(), password);
      onSuccess(me.username);
    } catch (e) {
      if (e instanceof ApiError && e.status === 429) setErr("Too many failed attempts. Wait a minute and try again.");
      else if (e instanceof ApiError && e.status === 401) setErr("Wrong username or password.");
      else setErr(errorMessage(e));
      setBusy(false);
    }
  };

  return (
    <div className="grid min-h-full place-items-center px-4">
      <form onSubmit={submit} className="card w-full max-w-sm animate-fade-in p-7 shadow-glow">
        <div className="mb-6 flex flex-col items-center text-center">
          <Logo className="mb-3 h-12 w-12 drop-shadow-[0_6px_20px_rgba(249,115,22,0.35)]" />
          <h1 className="text-lg font-semibold text-slate-100">WireFox</h1>
          <p className="mt-0.5 text-xs text-slate-500">Control Panel</p>
        </div>
        <div className="space-y-3">
          <Field label="Username">
            {(id) => (
              <input
                id={id}
                className="input"
                value={username}
                autoFocus
                autoComplete="username"
                onChange={(e) => setUsername(e.target.value)}
              />
            )}
          </Field>
          <Field label="Password">
            {(id) => (
              <input
                id={id}
                className="input"
                type="password"
                value={password}
                autoComplete="current-password"
                onChange={(e) => setPassword(e.target.value)}
              />
            )}
          </Field>
          {err && (
            <p className="text-sm text-rose-400" role="alert">
              {err}
            </p>
          )}
          <button type="submit" disabled={busy || !username.trim() || !password} className="btn-primary w-full">
            {busy ? <Spinner /> : <LogIn className="h-4 w-4" />} Sign in
          </button>
        </div>
      </form>
    </div>
  );
}
