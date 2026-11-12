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
  if (!Number.isInteger(decimals) || decimals < 0) throw new Error(`decimals must be a non-negative integer: ${decimals}`);
  const scale = 10n ** BigInt(decimals);
  const value = BigInt(units);
  const int = (value / scale).toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  // With no decimals there is no fraction; padStart(0) would leave "0".
  const frac = decimals === 0 ? "" : (value % scale).toString().padStart(decimals, "0");
  return { int, frac };
}

/** Build-time self-check: the page calls this, so a broken formatter fails the build. */
export function selfCheck(): void {
  const one = formatAmount((10n ** 24n + 5n * 10n ** 23n).toString());
  if (one.int !== "1" || one.frac !== "500000000000000000000000") throw new Error("formatAmount: 1.5 coins");
  if (formatAmount((1234567n * 10n ** 24n).toString()).int !== "1,234,567") throw new Error("formatAmount: grouping");
  if (formatAmount("0").frac.length !== COIN_DECIMALS) throw new Error("formatAmount: zero keeps 24 digits");
  // One base unit: the 24th digit survives, which a float (53-bit mantissa) cannot do.
  if (formatAmount("1").frac !== "000000000000000000000001") throw new Error("formatAmount: one base unit");
  const max = (2n ** 256n - 1n).toString();
  const big = formatAmount(max);
  if (big.int.replaceAll(",", "") + big.frac !== max) throw new Error("formatAmount: U256::MAX round-trips");
  // spec/address-vectors.json: 59 data symbols, so 14 groups of four and one of three.
  const text = "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f";
  const addr = chunkAddress(text);
  if (addr.prefix !== "maya1" || addr.chunks[0] !== "qq65" || addr.chunks.length !== 15 || addr.chunks[14] !== "36f") {
    throw new Error("chunkAddress: prefix and groups of four");
  }
  if (addr.prefix + addr.chunks.join("") !== text) throw new Error("chunkAddress: lossless");
}
