/**
 * Per-origin connect permission. A site sees no address and can ask for no
 * signature until the person approved that exact origin (scheme, host, port).
 *
 * The origin always comes from the browser's `sender` record for the content
 * script, never from anything the page put in a message.
 */

/** origin -> granted account indices. Immutable: every change returns a new map. */
export type Grants = Readonly<Record<string, readonly number[]>>;

const LOCAL_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * The origin a grant is keyed by, or null for a page that may not connect at
 * all: anything but https, except plain http on a loopback host for local dapp
 * development. A `file:`/`data:`/opaque origin has no stable identity to grant.
 */
export function grantableOrigin(url: string | undefined): string | null {
  if (!url) return null;
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol === "https:") return parsed.origin;
  if (parsed.protocol === "http:" && LOCAL_HOSTS.has(parsed.hostname)) return parsed.origin;
  return null;
}

export function accountsFor(grants: Grants, origin: string): readonly number[] {
  return Object.hasOwn(grants, origin) ? grants[origin] : [];
}

export function isConnected(grants: Grants, origin: string): boolean {
  return accountsFor(grants, origin).length > 0;
}

export function grant(grants: Grants, origin: string, accounts: readonly number[]): Grants {
  if (accounts.length === 0) throw new Error("a grant names at least one account");
  return { ...grants, [origin]: [...accounts] };
}

export function revoke(grants: Grants, origin: string): Grants {
  const next = { ...grants };
  delete next[origin];
  return next;
}
