// The homepage's decisions that are not markup, kept here so node --test can
// pin them (home.test.ts): which doors the hero offers in which network state,
// and how a tile words a figure it could not refresh.

export interface Cta { label: string; href: string }

/**
 * While the testnet is paused nothing can be mined, so the first button shows
 * the run that finished instead of inviting people to a stopped network.
 * Flipping `testnet.paused` brings "Start mining" back with no other edit.
 */
export function heroCtas(paused: boolean): { primary: Cta; secondary: Cta } {
  const secondary = { label: "Run a local devnet", href: "/guides/quickstart/" };
  return paused
    ? { primary: { label: "See the 7-day run", href: "/health/" }, secondary }
    : { primary: { label: "Start mining", href: "/guides/start-mining/" }, secondary };
}

/** Whole percent, rounded down so a 99.6 % run never reads as 100 %. */
export function uptimePercent(ratio: number): string {
  // The epsilon absorbs binary noise (0.29 * 100 = 28.999…), not real downtime.
  return `${Math.floor(ratio * 100 + 1e-9)}%`;
}

/** Polls in a row that must fail before a tile stops showing its last value as current. */
export const FAILS_BEFORE_UNREACHABLE = 2;

export interface Reading { value: string; at: Date }

/** What a live tile says once its source stopped answering: never a blank, never a stale value passed off as current. */
export function unreachableLabel(last: Reading | null): string {
  if (!last) return "unreachable";
  const at = last.at.toISOString().slice(0, 16).replace("T", " ");
  return `unreachable, last known ${last.value} ${at} UTC`;
}
