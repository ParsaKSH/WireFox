// Helpers for the SOCKS credentials shown on the Users page.

const PASSWORD_ALPHABET = "abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

export function generatePassword(length = 20): string {
  const bytes = crypto.getRandomValues(new Uint8Array(length));
  // 256 is not a multiple of the alphabet size; the slight bias doesn't matter at this length.
  return Array.from(bytes, (b) => PASSWORD_ALPHABET[b % PASSWORD_ALPHABET.length]).join("");
}

// The port of a listen address such as "0.0.0.0:1080" or "[::]:1080".
export function portOf(listen: string | undefined): string | null {
  if (!listen) return null;
  const i = listen.lastIndexOf(":");
  return i >= 0 && i < listen.length - 1 ? listen.slice(i + 1) : null;
}

export function socksUrl(username: string, password: string | null, host: string, port: string | null): string {
  const h = host.includes(":") && !host.startsWith("[") ? `[${host}]` : host;
  const auth = encodeURIComponent(username) + (password !== null ? `:${encodeURIComponent(password)}` : "");
  return `socks5://${auth}@${h}${port ? `:${port}` : ""}`;
}
