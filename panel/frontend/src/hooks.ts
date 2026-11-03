import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "./api";

export interface Resource<T> {
  data: T | undefined;
  error: string | null;
  loading: boolean;
  reload: () => Promise<void>;
  // Replaces the cached value, e.g. with the object a mutation returned. An
  // updater function gets the current value, so a mutation that finishes after
  // a poll merges into that poll's answer rather than into the data it started
  // from; it does nothing while nothing has loaded yet.
  set: (next: T | ((current: T) => T)) => void;
}

// How often views poll: live tables, the usual status refresh, slower lists.
export const POLL_LIVE_MS = 3000;
export const POLL_MS = 5000;
export const POLL_SLOW_MS = 10000;

// Loads data on mount and whenever `load` changes identity (wrap it in
// useCallback with its parameters as dependencies), and with an interval polls
// it while the browser tab is visible. A failed refresh keeps the last good
// data so tables don't vanish during a short outage; `error` says what failed.
export function useResource<T>(load: () => Promise<T>, intervalMs?: number): Resource<T> {
  const [data, setData] = useState<T>();
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const seq = useRef(0);

  const reload = useCallback(async () => {
    const mine = ++seq.current;
    setLoading(true);
    try {
      const v = await load();
      if (mine !== seq.current) return;
      setData(v);
      setError(null);
    } catch (e) {
      if (mine !== seq.current) return;
      setError(errorMessage(e));
    } finally {
      if (mine === seq.current) setLoading(false);
    }
  }, [load]);

  useEffect(() => {
    void reload();
    const tick = () => {
      if (document.visibilityState === "visible") void reload();
    };
    const timer = intervalMs ? window.setInterval(tick, intervalMs) : undefined;
    if (intervalMs) document.addEventListener("visibilitychange", tick);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
      // Drop any answer still in flight.
      seq.current++;
      setLoading(false);
    };
  }, [reload, intervalMs]);

  const set = useCallback((next: T | ((current: T) => T)) => {
    seq.current++;
    setData((current) => {
      if (!(next instanceof Function)) return next;
      return current === undefined ? current : next(current);
    });
    setError(null);
    setLoading(false);
  }, []);

  return { data, error, loading, reload, set };
}

// Re-renders every `ms` so relative times ("3m ago") stay current.
export function useTicker(ms: number) {
  const [, setTick] = useState(0);
  useEffect(() => {
    const t = window.setInterval(() => setTick((n) => n + 1), ms);
    return () => window.clearInterval(t);
  }, [ms]);
}
