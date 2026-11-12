// Formatting helpers for the component kit (docs/site/src/styles/components.css).
// Money is exact: amounts arrive as base-unit decimal strings and are split with
// BigInt, never through a float. The 24 is maya-amount's COIN_DECIMALS
// (crates/amount/src/lib.rs); a change there is a change here.

export const COIN_DECIMALS = 24;

/** Characters per group in a displayed address (a human compares four at a time). */
export const ADDRESS_CHUNK = 4;

/** Missing figure: the site shows a dash, never a zero or an estimate. */
export const MISSING = "—";

/**
 * Splits a Bech32m address into its human-readable prefix (up to and including
 * the last "1", which is the separator: the Bech32 data alphabet has no "1")
 * and fixed-size groups of the data part.
 */
export function chunkAddress(address: string, size = ADDRESS_CHUNK): { prefix: string; chunks: string[] } {
  if (!Number.isInteger(size) || size < 1) throw new Error(`chunk size must be a positive integer: ${size}`);
  const sep = address.lastIndexOf("1");
  const prefix = sep > 0 ? address.slice(0, sep + 1) : "";
  const body = address.slice(prefix.length);
  const chunks: string[] = [];
  for (let i = 0; i < body.length; i += size) chunks.push(body.slice(i, i + size));
  return { prefix, chunks };
}

/**
 * Formats a base-unit integer string as whole and fractional parts. The
 * fraction keeps every one of the `decimals` digits, so no precision is hidden.
 */
export function formatAmount(units: string, decimals = COIN_DECIMALS): { int: string; frac: string } {
  if (!/^\d+$/.test(units)) throw new Error(`amount must be a base-unit integer string: ${units}`);
  const scale = 10n ** BigInt(decimals);
  const value = BigInt(units);
  const int = (value / scale).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  const frac = (value % scale).toString().padStart(decimals, "0");
  return { int, frac };
}

/** Build-time self-check: the page calls this, so a broken formatter fails the build. */
export function selfCheck(): void {
  const one = formatAmount((10n ** 24n + 5n * 10n ** 23n).toString());
  if (one.int !== "1" || one.frac !== "500000000000000000000000") throw new Error("formatAmount: 1.5 coins");
  if (formatAmount((1234567n * 10n ** 24n).toString()).int !== "1,234,567") throw new Error("formatAmount: grouping");
  if (formatAmount("0").frac.length !== COIN_DECIMALS) throw new Error("formatAmount: zero keeps 24 digits");
  const addr = chunkAddress("maya1qz3k9w7d4e8f2h6j0m5pr3t7v9x2y4a6c8e0gq");
  if (addr.prefix !== "maya1" || addr.chunks[0] !== "qz3k" || addr.chunks.length !== 10) {
    throw new Error("chunkAddress: prefix and groups of four");
  }
}
