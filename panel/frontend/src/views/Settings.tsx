import { useEffect, useState, type FormEvent } from "react";
import { KeyRound, Save } from "lucide-react";
import { api, errorMessage } from "../api";
import { useResource } from "../hooks";
import type { Settings } from "../types";
import { ErrorState, Field, LoadingState, PageHeader, SectionTitle, Spinner, useToast } from "../ui";

export function SettingsView() {
  return (
    <div>
      <PageHeader title="Settings" subtitle="Panel addresses and your account" />
      <div className="grid grid-cols-1 gap-6 xl:grid-cols-2">
        <AddressesCard />
        <PasswordCard />
      </div>
    </div>
  );
}

function AddressesCard() {
  const toast = useToast();
  const settings = useResource(api.settings);
  const [form, setForm] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (settings.data) setForm(settings.data);
  }, [settings.data]);

  const dirty =
    !!form &&
    !!settings.data &&
    (form.agent_public_addr !== settings.data.agent_public_addr ||
      form.socks_public_host !== settings.data.socks_public_host);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    if (!form) return;
    setSaving(true);
    try {
      const next = {
        agent_public_addr: form.agent_public_addr.trim(),
        socks_public_host: form.socks_public_host.trim(),
      };
      await api.saveSettings(next);
      settings.set(next);
      toast("success", "Settings saved");
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div>
      <SectionTitle>Public addresses</SectionTitle>
      <div className="card p-5">
        {!form ? (
          settings.error ? (
            <ErrorState message={settings.error} onRetry={settings.reload} />
          ) : (
            <LoadingState />
          )
        ) : (
          <form onSubmit={save} className="space-y-4">
            <Field
              label="Agent address"
              hint={
                <>
                  Host and port where exit-nodes reach the panel's gRPC listener, for example{" "}
                  <span className="font-mono">panel.example.com:50051</span>. It is written into every node key, so keys
                  issued before a change keep pointing at the old address until you issue new ones.
                </>
              }
            >
              {(id) => (
                <input
                  id={id}
                  className="input font-mono"
                  value={form.agent_public_addr}
                  placeholder="panel.example.com:50051"
                  spellCheck={false}
                  onChange={(e) => setForm({ ...form, agent_public_addr: e.target.value })}
                />
              )}
            </Field>
            <Field
              label="SOCKS host"
              hint="Host name or IP that clients use to reach the SOCKS listener. Only used to show ready-made socks5:// addresses on the Users page; empty uses the host this panel is opened on."
            >
              {(id) => (
                <input
                  id={id}
                  className="input font-mono"
                  value={form.socks_public_host}
                  placeholder={window.location.hostname}
                  spellCheck={false}
                  onChange={(e) => setForm({ ...form, socks_public_host: e.target.value })}
                />
              )}
            </Field>
            <div className="flex justify-end">
              <button type="submit" className="btn-primary" disabled={!dirty || saving}>
                {saving ? <Spinner /> : <Save className="h-4 w-4" />} Save
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
}

function PasswordCard() {
  const toast = useToast();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const [busy, setBusy] = useState(false);

  const tooShort = next.length > 0 && next.length < 8;
  const mismatch = repeat.length > 0 && next !== repeat;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    try {
      await api.changePassword(current, next);
      toast("success", "Password changed");
      setCurrent("");
      setNext("");
      setRepeat("");
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <SectionTitle>Change password</SectionTitle>
      <form onSubmit={submit} className="card space-y-4 p-5">
        <Field label="Current password">
          {(id) => (
            <input
              id={id}
              className="input"
              type="password"
              autoComplete="current-password"
              value={current}
              onChange={(e) => setCurrent(e.target.value)}
            />
          )}
        </Field>
        <Field label="New password" error={tooShort ? "At least 8 characters" : undefined}>
          {(id) => (
            <input
              id={id}
              className="input"
              type="password"
              autoComplete="new-password"
              value={next}
              onChange={(e) => setNext(e.target.value)}
            />
          )}
        </Field>
        <Field label="Repeat new password" error={mismatch ? "The passwords differ" : undefined}>
          {(id) => (
            <input
              id={id}
              className="input"
              type="password"
              autoComplete="new-password"
              value={repeat}
              onChange={(e) => setRepeat(e.target.value)}
            />
          )}
        </Field>
        <div className="flex justify-end">
          <button
            type="submit"
            className="btn-primary"
            disabled={busy || !current || next.length < 8 || next !== repeat}
          >
            {busy ? <Spinner /> : <KeyRound className="h-4 w-4" />} Change password
          </button>
        </div>
      </form>
    </div>
  );
}
