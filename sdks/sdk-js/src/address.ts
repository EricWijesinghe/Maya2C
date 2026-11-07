/**
 * Bech32m text form of an account address (BIP-350, ADR-081). The wire layout
 * matches `crates/address`: `<hrp>1` + version symbol + 52 payload symbols +
 * 6 checksum symbols. Shared vectors live in `spec/address-vectors.json`.
 */

export type AddressNetwork = "mainnet" | "testnet" | "devnet";

export type AddressErrorKind =
  | "Length"
  | "MixedCase"
  | "NoSeparator"
  | "EmptyHrp"
  | "InvalidCharacter"
  | "BadChecksum"
  | "WrongNetwork"
  | "UnknownNetwork"
  | "Version"
  | "Payload";

/** A refused address. `kind` is the same name the Rust crate's error uses. */
export class AddressError extends Error {
  readonly kind: AddressErrorKind;

  constructor(kind: AddressErrorKind, message: string) {
    super(message);
    this.name = "AddressError";
    this.kind = kind;
  }
}

const HRP: Record<AddressNetwork, string> = { mainnet: "maya", testnet: "tmaya", devnet: "dmaya" };
const CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l";
const GENERATOR = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];
const BECH32M_CONST = 0x2bc830a3;
const CHECKSUM_LEN = 6;
const MAX_LEN = 90;
const VERSION = 0;
const PAYLOAD_SYMBOLS = 52;
const DATA_SYMBOLS = 1 + PAYLOAD_SYMBOLS;
export const ADDRESS_LEN = 32;

function polymod(values: readonly number[]): number {
  let chk = 1;
  for (const v of values) {
    const top = chk >>> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    GENERATOR.forEach((g, i) => {
      if ((top >>> i) & 1) chk ^= g;
    });
  }
  return chk >>> 0;
}

function expandHrp(hrp: string): number[] {
  const codes = [...hrp].map((c) => c.charCodeAt(0));
  return [...codes.map((c) => c >>> 5), 0, ...codes.map((c) => c & 31)];
}

function toSymbols(bytes: Uint8Array): number[] {
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const b of bytes) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      out.push((acc >>> bits) & 31);
    }
    acc &= (1 << bits) - 1;
  }
  if (bits > 0) out.push((acc << (5 - bits)) & 31);
  return out;
}

function fromSymbols(symbols: readonly number[]): Uint8Array {
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const s of symbols) {
    acc = (acc << 5) | s;
    bits += 5;
    while (bits >= 8) {
      bits -= 8;
      out.push((acc >>> bits) & 0xff);
    }
    acc &= (1 << bits) - 1;
  }
  // Non-zero leftover padding means a non-canonical string: refuse it.
  if (bits >= 5 || acc !== 0 || out.length !== ADDRESS_LEN) {
    throw new AddressError("Payload", "address payload is malformed");
  }
  return Uint8Array.from(out);
}

function createChecksum(hrp: string, data: readonly number[]): number[] {
  const mod = polymod([...expandHrp(hrp), ...data, 0, 0, 0, 0, 0, 0]) ^ BECH32M_CONST;
  return Array.from({ length: CHECKSUM_LEN }, (_, i) => (mod >>> (5 * (CHECKSUM_LEN - 1 - i))) & 31);
}

/** Splits a Bech32m string into its lowercased HRP and data symbols (checksum removed). */
function split(text: string): { hrp: string; data: number[] } {
  if (text.length > MAX_LEN) throw new AddressError("Length", `address is ${text.length} characters; the limit is 90`);
  const hasLower = /[a-z]/.test(text);
  const hasUpper = /[A-Z]/.test(text);
  if (hasLower && hasUpper) throw new AddressError("MixedCase", "address mixes upper and lower case");
  const lower = text.toLowerCase();
  const sep = lower.lastIndexOf("1");
  if (sep < 0) throw new AddressError("NoSeparator", "address has no '1' separator");
  const hrp = lower.slice(0, sep);
  if (hrp.length === 0) throw new AddressError("EmptyHrp", "address has no network prefix before the '1'");
  for (const c of hrp) {
    const code = c.charCodeAt(0);
    if (code < 33 || code > 126) throw new AddressError("InvalidCharacter", `address contains ${JSON.stringify(c)}`);
  }
  const body = lower.slice(sep + 1);
  if (body.length !== DATA_SYMBOLS + CHECKSUM_LEN) throw new AddressError("Payload", "address payload is malformed");
  const data = [...body].map((c) => {
    const at = CHARSET.indexOf(c);
    if (at < 0) throw new AddressError("InvalidCharacter", `address contains ${JSON.stringify(c)}`);
    return at;
  });
  if (polymod([...expandHrp(hrp), ...data]) !== BECH32M_CONST) {
    throw new AddressError("BadChecksum", "address checksum does not match; a character is mistyped");
  }
  return { hrp, data: data.slice(0, DATA_SYMBOLS) };
}

function parse(text: string): { hrp: string; id: Uint8Array } {
  const { hrp, data } = split(text);
  if (data[0] !== VERSION) throw new AddressError("Version", `address format version ${data[0]} is not known`);
  return { hrp, id: fromSymbols(data.slice(1)) };
}

/** Renders a 32-byte account id as its Bech32m text form on `network`. */
export function encodeAddress(network: AddressNetwork, id: Uint8Array): string {
  if (id.length !== ADDRESS_LEN) throw new RangeError(`an account id is ${ADDRESS_LEN} bytes, got ${id.length}`);
  const data = [VERSION, ...toSymbols(id)];
  const checksum = createChecksum(HRP[network], data);
  return `${HRP[network]}1${[...data, ...checksum].map((s) => CHARSET[s]).join("")}`;
}

/**
 * Decodes an address that must be for `expected`.
 *
 * @throws {AddressError} with `kind` `WrongNetwork` for a valid address on another network.
 */
export function decodeAddress(expected: AddressNetwork, text: string): Uint8Array {
  const { hrp, id } = parse(text);
  if (hrp !== HRP[expected]) {
    throw new AddressError("WrongNetwork", `address is for network prefix '${hrp}', but this network expects '${HRP[expected]}'`);
  }
  return id;
}

/** Decodes an address for whichever known network its prefix names. */
export function decodeAnyAddress(text: string): { network: AddressNetwork; id: Uint8Array } {
  const { hrp, id } = parse(text);
  const network = (Object.keys(HRP) as AddressNetwork[]).find((n) => HRP[n] === hrp);
  if (network === undefined) throw new AddressError("UnknownNetwork", `address prefix '${hrp}' names no known network`);
  return { network, id };
}
