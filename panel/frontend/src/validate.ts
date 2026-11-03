// Form checks that mirror what the core and the panel accept, so mistakes show
// up next to the field rather than as an error toast after saving.

// Node ids and usernames: up to 64 letters, digits, dots, dashes and
// underscores. A name made only of dots can't be used: "." and ".." are
// collapsed as path segments, so its API URLs would point somewhere else.
export function idError(id: string): string | undefined {
  if (id.length > 64) return "At most 64 characters";
  if (!/^[A-Za-z0-9._-]*$/.test(id)) return "Letters, digits, dot, dash and underscore only";
  if (/^\.+$/.test(id)) return "Must not be made only of dots";
  return undefined;
}

// A whole number between min and max, typed as text.
export function intError(v: string, min: number, max: number): string | undefined {
  if (!/^\d+$/.test(v)) return "A whole number";
  const n = Number(v);
  return n < min || n > max ? `Between ${min} and ${max}` : undefined;
}
