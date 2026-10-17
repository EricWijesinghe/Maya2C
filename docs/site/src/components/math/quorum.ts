// DAG-BFT quorum arithmetic, equal weights (ADR-039). Stake-weighted
// committees (ADR-040) replace the head count with weight W; the same
// algebra applies with n read as W.

/** Largest f with n >= 3f + 1, i.e. floor((n - 1) / 3). */
export function faults(n: number): number {
  return Math.floor((n - 1) / 3);
}

/** The rule in use: a certificate or round needs n - f votes. */
export function quorum(n: number): number {
  return n - faults(n);
}

/** The retired rule, 2f + 1, kept only to show where it fails. */
export function legacyQuorum(n: number): number {
  return 2 * faults(n) + 1;
}

/** Fewest members two quorums can share: 2q - n. */
export function minOverlap(q: number, n: number): number {
  return 2 * q - n;
}

/** Safety: every overlap must contain at least f + 1 members, so one is honest. */
export function isSafe(n: number, q: number): boolean {
  return minOverlap(q, n) >= faults(n) + 1;
}
