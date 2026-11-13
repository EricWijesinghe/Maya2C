/** Hex and amount encodings shared by the background and the popup. */

/** Base units in one coin: 10^24 (ADR-068, `spec/tests/units.json`). */
export const COIN_DECIMALS = 24;
const ONE_COIN = 10n ** BigInt(COIN_DECIMALS);
const MAX_U256 = (1n << 256n) - 1n;

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function fromHex(text: string): Uint8Array {
  const clean = text.startsWith("0x") ? text.slice(2) : text;
  if (clean.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(clean)) {
    throw new Error("not hex");
  }
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.slice(2 * i, 2 * i + 2), 16);
  return out;
}

/** A base-unit decimal string ("12345") as a bigint, refusing anything else. */
export function parseBaseUnits(text: string): bigint {
  if (!/^[0-9]{1,78}$/.test(text)) throw new Error("amount must be a whole number of base units");
  const value = BigInt(text);
  if (value > MAX_U256) throw new Error("amount does not fit in 256 bits");
  return value;
}

/** "1.5" coins as base units. Refuses more than 24 decimals rather than rounding. */
export function parseCoins(text: string): bigint {
  const match = /^([0-9]{1,54})(?:\.([0-9]{1,24}))?$/.exec(text.trim());
  if (!match) throw new Error("amount must look like 1.25 (at most 24 decimals)");
  const whole = BigInt(match[1]);
  const frac = BigInt((match[2] ?? "").padEnd(COIN_DECIMALS, "0"));
  const value = whole * ONE_COIN + frac;
  if (value > MAX_U256) throw new Error("amount does not fit in 256 bits");
  return value;
}

/** Base units as a coin string with trailing zeros trimmed: 1500…0 -> "1.5". */
export function formatCoins(value: bigint): string {
  const whole = value / ONE_COIN;
  const frac = (value % ONE_COIN).toString().padStart(COIN_DECIMALS, "0").replace(/0+$/, "");
  return frac ? `${whole}.${frac}` : whole.toString();
}

/** A 256-bit value as 32 little-endian bytes: the node's `put_amount`. */
export function toLe32(value: bigint): Uint8Array {
  if (value < 0n || value > MAX_U256) throw new RangeError("value out of 256-bit range");
  const out = new Uint8Array(32);
  let v = value;
  for (let i = 0; i < 32; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

/** Overwrites a buffer. JS has no guaranteed zeroization; this is the most it allows. */
export function wipe(...buffers: (Uint8Array | undefined)[]): void {
  for (const b of buffers) b?.fill(0);
}
