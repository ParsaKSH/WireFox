import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "./api";

export interface Resource<T> {
  data: T | undefined;
  error: string | null;
  loading: boolean;
  reload: () => Promise<void>;
  // Replaces the cached value, e.g. with the object a mutation returned.
  set: (data: T) => void;
}

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

  const set = useCallback((v: T) => {
    seq.current++;
    setData(v);
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
