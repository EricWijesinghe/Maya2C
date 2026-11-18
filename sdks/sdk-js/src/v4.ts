// Read, write and describe the v4 transaction payloads (ADR-135).
//
// A payload is `tag(1) || fields` as crates/node/src/core/*_payload.rs writes
// it: addresses and ids 32 bytes, U256 32 bytes little-endian, u64/u32
// little-endian, a name label one length byte then ASCII.
//
// This module reads and previews payloads; transactions are still built and
// signed by the Rust wallet. The words `describePayload` returns are the
// node's own (core::describe). spec/tests/v4-client-vectors.json, written by
// the node, pins both the bytes and the words, and the tests fail on drift.
//
// Fields are strings: integers as decimal, bytes as lowercase hex, labels and
// symbols as ASCII -- the same shape as the vectors, so a u64 or U256 never
// passes through a JS number.

export type Fields = Record<string, string>;

export class PayloadError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "PayloadError";
  }
}

type Kind = "b32" | "u256" | "u64" | "u32" | "u8" | "symbol" | "label";

const INT_LEN: Record<"u8" | "u32" | "u64" | "u256", number> = { u8: 1, u32: 4, u64: 8, u256: 32 };
const SYMBOL_LEN = 8;

// Maps, not object literals: a plain-object lookup would find inherited keys
// such as "constructor" or "__proto__" and write a byte nobody chose.
const FORMS = new Map<string, [number, [string, Kind][]]>(Object.entries({
  register_capped_asset: [55, [["symbol", "symbol"], ["total_supply", "u256"], ["cap", "u256"], ["authority", "b32"]]],
  mint_asset: [56, [["asset", "b32"], ["to", "b32"], ["amount", "u256"]]],
  burn_asset: [57, [["asset", "b32"], ["amount", "u256"]]],
  renounce_mint: [58, [["asset", "b32"]]],
  create_collection: [59, [["max_items", "u64"], ["metadata_hash", "b32"]]],
  mint_item: [60, [["collection", "b32"], ["metadata_hash", "b32"]]],
  transfer_item: [61, [["collection", "b32"], ["index", "u64"], ["to", "b32"]]],
  burn_item: [62, [["collection", "b32"], ["index", "u64"]]],
  commit_name: [63, [["commitment", "b32"]]],
  register_name: [64, [["label", "label"], ["salt", "b32"], ["periods", "u8"], ["target", "b32"]]],
  renew_name: [65, [["label", "label"], ["periods", "u8"]]],
  update_name: [66, [["label", "label"]]],
  create_schedule: [67, [["payee", "b32"], ["amount", "u256"], ["first_due", "u64"], ["interval", "u64"], ["runs", "u32"]]],
  cancel_schedule: [68, [["schedule_id", "b32"]]],
} satisfies Record<string, [number, [string, Kind][]]>));
const BY_TAG = new Map([...FORMS].map(([label, [tag]]) => [tag, label]));
// update_name ops: name -> [byte, carries an address]
const OPS = new Map<string, [number, boolean]>([
  ["set_target", [0x01, true]],
  ["set_reverse", [0x03, false]],
  ["transfer", [0x04, true]],
]);
const OPS_BY_BYTE = new Map([...OPS].map(([name, [byte]]) => [byte, name]));

/** The node's label rule: 3-63 bytes of a-z, 0-9, '-', no edge or double '-'. */
export function validateLabel(label: string): void {
  if (label.length < 3 || label.length > 63) throw new PayloadError("a label is 3 to 63 bytes");
  if (!/^[a-z0-9-]+$/.test(label)) throw new PayloadError("a label is a-z, 0-9 and '-' only");
  if (label.startsWith("-") || label.endsWith("-")) throw new PayloadError("a label does not start or end with '-'");
  if (label.includes("--")) throw new PayloadError("a label has no '--'");
}

const hexOf = (bytes: Uint8Array): string => Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");

function bytesOf(hex: string, name: string): number[] {
  if (!/^([0-9a-f]{2}){32}$/.test(hex)) throw new PayloadError(`${name} is not 32 bytes of lowercase hex`);
  return Array.from({ length: 32 }, (_, i) => parseInt(hex.slice(i * 2, i * 2 + 2), 16));
}

function put(out: number[], fields: Fields, name: string, kind: Kind): void {
  const value = fields[name];
  if (value === undefined) throw new PayloadError(`missing field ${name}`);
  if (kind === "b32") {
    out.push(...bytesOf(value, name));
  } else if (kind === "symbol") {
    // The node's symbol rule (state::asset::validate_symbol): 1-8 of A-Z, 0-9.
    if (!/^[A-Z0-9]{1,8}$/.test(value)) throw new PayloadError("a symbol is 1 to 8 of A-Z and 0-9");
    const raw = Array.from(value, (c) => c.charCodeAt(0));
    out.push(...raw, ...new Array<number>(SYMBOL_LEN - raw.length).fill(0));
  } else if (kind === "label") {
    validateLabel(value);
    out.push(value.length, ...Array.from(value, (c) => c.charCodeAt(0)));
  } else {
    if (!/^[0-9]+$/.test(value)) throw new PayloadError(`${name} is not a decimal integer`);
    let n = BigInt(value);
    const len = INT_LEN[kind];
    if (n >= 1n << BigInt(8 * len)) throw new PayloadError(`${name} does not fit ${kind}`);
    for (let i = 0; i < len; i++) {
      out.push(Number(n & 0xffn));
      n >>= 8n;
    }
  }
}

/** The payload bytes of form `label` with `fields`. */
export function encodePayload(label: string, fields: Fields): Uint8Array {
  const form = FORMS.get(label);
  if (form === undefined) throw new PayloadError(`${label} is not a v4 form`);
  const out = [form[0]];
  for (const [name, kind] of form[1]) put(out, fields, name, kind);
  if (label === "update_name") {
    const op = OPS.get(fields.op ?? "");
    if (op === undefined) throw new PayloadError(`update op ${fields.op} is not in this build`);
    out.push(op[0]);
    if (op[1]) put(out, fields, "address", "b32");
  }
  return Uint8Array.from(out);
}

class Reader {
  at = 0;
  constructor(readonly data: Uint8Array) {}
  take(n: number): Uint8Array {
    if (this.at + n > this.data.length) throw new PayloadError("payload is truncated");
    const chunk = this.data.subarray(this.at, this.at + n);
    this.at += n;
    return chunk;
  }
}

function get(reader: Reader, fields: Fields, name: string, kind: Kind): void {
  if (kind === "b32") {
    fields[name] = hexOf(reader.take(32));
  } else if (kind === "symbol") {
    // As core::describe reads it: UTF-8, lossy, over all eight bytes, then
    // trailing NULs trimmed. A NUL inside stays, so no two symbols read alike.
    fields[name] = new TextDecoder("utf-8").decode(reader.take(SYMBOL_LEN)).replace(/\0+$/, "");
  } else if (kind === "label") {
    const len = reader.take(1)[0] ?? 0;
    const label = String.fromCharCode(...reader.take(len));
    validateLabel(label);
    fields[name] = label;
  } else {
    const raw = reader.take(INT_LEN[kind]);
    let n = 0n;
    for (let i = raw.length - 1; i >= 0; i--) n = (n << 8n) | BigInt(raw[i] ?? 0);
    fields[name] = n.toString();
  }
}

/** `[label, fields]` of a v4 payload. Refuses unknown tags and trailing bytes. */
export function decodePayload(payload: Uint8Array): [string, Fields] {
  const tag = payload[0];
  if (tag === undefined) throw new PayloadError("payload is empty");
  const label = BY_TAG.get(tag);
  const form = label === undefined ? undefined : FORMS.get(label);
  if (label === undefined || form === undefined) throw new PayloadError(`tag ${tag} is not a v4 form`);
  const reader = new Reader(payload.subarray(1));
  const fields: Fields = {};
  for (const [name, kind] of form[1]) get(reader, fields, name, kind);
  if (label === "update_name") {
    const op = OPS_BY_BYTE.get(reader.take(1)[0] ?? -1);
    if (op === undefined) throw new PayloadError("name update op is not in this build");
    fields.op = op;
    if (OPS.get(op)?.[1]) get(reader, fields, "address", "b32");
  }
  if (reader.at !== reader.data.length) throw new PayloadError("trailing bytes after the payload");
  return [label, fields];
}

/** `1234567` reads `1,234,567`, as core::describe::group_digits. */
export function groupDigits(value: string): string {
  return value.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

const short = (hex: string): string => `${hex.slice(0, 4)}…${hex.slice(-4)}`;

export interface Description {
  summary: string;
  careful: boolean;
  details: [string, string][];
}

/**
 * The words a wallet shows for a v4 payload, read from its bytes. Use this
 * for a preview: it describes exactly what will be signed.
 */
export function describeBytes(payload: Uint8Array): Description {
  const [label, fields] = decodePayload(payload);
  return describePayload(label, fields);
}

/**
 * The words for `label` with `fields`, as the node words them. `fields` must
 * be what `decodePayload` returned, or a preview can disagree with the bytes
 * that get signed; `describeBytes` does both steps.
 */
export function describePayload(label: string, f: Fields): Description {
  const n = (k: string): string => groupDigits(f[k] ?? "");
  const d = (summary: string, careful: boolean, details: [string, string][] = []): Description => ({
    summary,
    careful,
    details,
  });
  const at = (k: string): string => f[k] ?? "";
  switch (label) {
    case "register_capped_asset":
      return d(`Register an asset with ${n("total_supply")} units now, all credited to you, and at most ${n("cap")} ever.`, true, [
        ["symbol", at("symbol")],
        ["mint authority", at("authority")],
      ]);
    case "mint_asset":
      return d(`Create ${n("amount")} new units of an asset for ${short(at("to"))}.`, true, [
        ["asset", at("asset")],
        ["pay to", at("to")],
      ]);
    case "burn_asset":
      return d(`Destroy ${n("amount")} of your units of an asset. This cannot be undone.`, true, [["asset", at("asset")]]);
    case "renounce_mint":
      return d("Give up minting this asset for good. Its cap falls to the current supply.", true, [["asset", at("asset")]]);
    case "create_collection":
      return d(`Open a collection of up to ${n("max_items")} unique items that only you can mint.`, true);
    case "mint_item":
      return d("Mint the next unique item in a collection you created.", false, [["collection", at("collection")]]);
    case "transfer_item":
      return d(`Send item ${at("index")} to ${short(at("to"))}.`, true, [
        ["collection", at("collection")],
        ["pay to", at("to")],
      ]);
    case "burn_item":
      return d(`Destroy item ${at("index")}. This cannot be undone.`, true, [["collection", at("collection")]]);
    case "commit_name":
      return d("Commit to a name. Free; you can reveal it after a short wait.", false);
    case "register_name":
      return d(`Register ${at("label")}.maya for ${at("periods")} year(s). The fee is burned.`, true, [["name target", at("target")]]);
    case "renew_name":
      return d(`Extend ${at("label")}.maya by ${at("periods")} year(s). The fee is burned.`, true);
    case "update_name":
      if (f.op === "set_target") {
        return d(`Point ${at("label")}.maya at ${short(at("address"))}.`, true, [["name target", at("address")]]);
      }
      if (f.op === "set_reverse") return d(`Show ${at("label")}.maya as the name of your address.`, false);
      if (f.op === "transfer") {
        return d(`Give ${at("label")}.maya to ${short(at("address"))}.`, true, [["new owner", at("address")]]);
      }
      throw new PayloadError(`update op ${f.op} is not in this build`);
    case "create_schedule":
      return d(
        `Pay ${n("amount")} ${at("runs")} times to ${short(at("payee"))}, every ${at("interval")} blocks from block ${at("first_due")}. All of it, and the run fees, leave your balance now.`,
        true,
        [["pay to", at("payee")]],
      );
    case "cancel_schedule":
      return d("Cancel a scheduled payment. What it has not paid yet comes back to you.", false, [["schedule", at("schedule_id")]]);
    default:
      throw new PayloadError(`${label} is not a v4 form`);
  }
}
