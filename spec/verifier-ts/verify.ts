// Independent verifier (Master Prompt 15 §3): replays spec/tests/ in a second
// language with its own BLAKE3 and checks every state root, fee and decode.
//
//   node spec/verifier-ts/verify.ts          # Node >= 22.18 runs .ts directly
//
// It is a second opinion, not a node. Signatures are NOT verified: a vector
// says whether its signature pair is valid, and this program trusts that flag,
// because ML-DSA and SLH-DSA are checked against their FIPS KATs elsewhere.
// Everything else — encoding, ids, addresses, the transfer rules, the root and
// the fee arithmetic — is recomputed here from the spec text, with BigInt for
// every u64 so no value is silently rounded.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { blake3, deriveKey, fromHex, selftest, toHex } from "./blake3.ts";

const TESTS = join(dirname(fileURLToPath(import.meta.url)), "..", "tests");
const U64_MAX = (1n << 64n) - 1n;
const load = (name: string) => JSON.parse(readFileSync(join(TESTS, name), "utf8"));
const le64 = (v: bigint) => Uint8Array.from({ length: 8 }, (_, i) => Number((v >> BigInt(8 * i)) & 0xffn));
const readLe64 = (b: Uint8Array, at: number) => b.subarray(at, at + 8).reduceRight((acc, x) => (acc << 8n) | BigInt(x), 0n);
const concat = (...parts: Uint8Array[]) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) (out.set(p, at), (at += p.length));
  return out;
};
const text = (s: string) => new TextEncoder().encode(s);

// ---------------------------------------------------------------- state root
interface Account { balance: bigint; nonce: bigint }
type State = Map<string, Account>; // key: address hex

const leaf = (addr: string, a: Account) => blake3(concat(Uint8Array.of(0), fromHex(addr), le64(a.balance), le64(a.nonce)));
const node = (l: Uint8Array, r: Uint8Array) => blake3(concat(Uint8Array.of(1), l, r));

function root(state: State): string {
  let level = [...state.keys()].sort().map((a) => leaf(a, state.get(a) as Account));
  if (level.length === 0) return "00".repeat(32);
  while (level.length > 1) {
    const next: Uint8Array[] = [];
    for (let i = 0; i < level.length; i += 2) next.push(i + 1 < level.length ? node(level[i], level[i + 1]) : level[i]);
    level = next;
  }
  return toHex(level[0]);
}

// --------------------------------------------------------- state transition
interface Tx { from: string; nonce: bigint; outputs: Array<[string, bigint]>; valid: boolean }

function applyTx(s: State, tx: Tx): string | null {
  if (!tx.valid) return "BadSignature";
  const sender = s.get(tx.from) ?? { balance: 0n, nonce: 0n };
  if (tx.nonce !== sender.nonce) return "InvalidNonce";
  let total = 0n;
  for (const [, amount] of tx.outputs) if ((total += amount) > U64_MAX) return "BalanceOverflow";
  if (sender.balance < total) return "InsufficientBalance";
  if (sender.nonce === U64_MAX) return "BalanceOverflow";
  s.set(tx.from, { balance: sender.balance - total, nonce: sender.nonce + 1n });
  for (const [to, amount] of tx.outputs) {
    const r = s.get(to) ?? { balance: 0n, nonce: 0n };
    if (r.balance + amount > U64_MAX) return "BalanceOverflow";
    s.set(to, { balance: r.balance + amount, nonce: r.nonce });
  }
  return null;
}

function stateTransitions(keys: Map<string, string>): number {
  const addr = (n: string) => keys.get(n) ?? n;
  const doc = load("state_transitions.json");
  for (const c of doc.cases) {
    const pre: State = new Map(c.pre.map((a: any) => [addr(a.account), { balance: BigInt(a.balance), nonce: BigInt(a.nonce) }]));
    check(root(pre) === c.pre_root, `${c.id}: pre_root`);
    const post: State = new Map([...pre].map(([k, v]) => [k, { ...v }]));
    let failure: [number, string] | null = null;
    c.block.forEach((t: any, i: number) => {
      if (failure) return;
      const tx: Tx = { from: addr(t.from), nonce: BigInt(t.nonce), valid: t.signature === "valid", outputs: t.outputs.map((o: any) => [addr(o.to), BigInt(o.amount)]) };
      const why = applyTx(post, tx);
      if (why) failure = [i, why];
    });
    // CON-4: a declared state root must be the one execution produces.
    if (failure === null && c.declared_state_root !== undefined && root(post) !== c.declared_state_root) {
      failure = [-1, "StateRootMismatch"];
    }
    if (c.expect.result === "ok") {
      check(failure === null, `${c.id}: rejected a valid block (${failure})`);
      check(root(post) === c.expect.post_root, `${c.id}: post_root`);
    } else {
      const at = c.expect.tx_index ?? -1;
      check(failure !== null && failure[0] === at && failure[1] === c.expect.error, `${c.id}: expected ${c.expect.error} at ${at}, got ${failure}`);
    }
  }
  return doc.cases.length;
}

// ---------------------------------------------------------------- encoding
const PK = 1952 + 32;
const SIG = 3309 + 7856;
const MAX_COLLECTION = 65536n;

/** Decodes a v5 transfer frame; returns null for any frame the spec rejects. */
function decode(b: Uint8Array): { io: Uint8Array; pk: Uint8Array; nonce: bigint; sig: Uint8Array } | null {
  if (b.length < 1 || b[0] !== 5) return null; // ENC-1 (this verifier covers v5 only)
  let at = 1;
  const count = (elem: number) => {
    if (at + 8 > b.length) return null;
    const n = readLe64(b, at);
    at += 8;
    if (n > MAX_COLLECTION || BigInt(b.length - at) < n * BigInt(elem)) return null; // ENC-6, ENC-7
    at += Number(n) * elem;
    return n;
  };
  if (count(36) === null || count(40) === null) return null;
  const io = b.subarray(1, at);
  if (at + PK + 9 > b.length) return null;
  const pk = b.subarray(at, at + PK);
  const nonce = readLe64(b, at + PK);
  at += PK + 8;
  const flag = b[at++];
  let sig = new Uint8Array(0);
  if (flag === 1) {
    sig = b.subarray(at, at + SIG);
    at += SIG;
  } else if (flag !== 0) return null; // ENC-4
  if (at !== b.length) return null; // ENC-5
  return { io, pk, nonce, sig };
}

/** ENC-8: `blake3(txid_domain ‖ 0x00 ‖ body ‖ signature)`. The id names no chain. */
function txid(body: Uint8Array, sig: Uint8Array): Uint8Array {
  return blake3(concat(text("custom-l1-node.txid.v1"), new Uint8Array([0]), body, sig));
}

function encoding(): number {
  const doc = load("encoding.json");
  for (const c of doc.cases) {
    const f = decode(fromHex(c.bytes));
    if (c.expect.result === "error") {
      check(f === null, `${c.id}: accepted a frame the spec rejects`);
      continue;
    }
    check(f !== null, `${c.id}: rejected a valid frame`);
    if (!f) continue;
    const body = concat(f.io, f.pk, le64(f.nonce));
    const signing = concat(text("custom-l1-node.tx.v4"), fromHex(c.chain_tag), body); // TX-3, TX-5
    check(toHex(blake3(signing)) === c.expect.signing_bytes_blake3, `${c.id}: signing bytes`);
    check(toHex(txid(body, f.sig)) === c.expect.txid, `${c.id}: txid`); // ENC-8
    check(toHex(blake3(concat(text("custom-l1-node.address.v3"), f.pk))) === c.expect.sender, `${c.id}: sender`);
    check(f.nonce === BigInt(c.expect.nonce), `${c.id}: nonce`);
  }
  return doc.cases.length;
}

// -------------------------------------------------------------------- fees
function fees(): number {
  const doc = load("fees.json");
  for (const c of doc.cases) {
    if (c.fn === "admit") {
      const ok = BigInt(c.max_fee) >= BigInt(c.base_fee) * BigInt(c.size_bytes); // FEE-5
      check(ok === (c.expect.result === "ok"), `${c.id}`);
    } else if (c.fn === "split") {
      const bps = BigInt(c.treasury_bps) < 10000n ? BigInt(c.treasury_bps) : 10000n;
      const treasury = (BigInt(c.base_fee_paid) * bps) / 10000n;
      check(treasury === BigInt(c.expect.treasury) && BigInt(c.base_fee_paid) - treasury === BigInt(c.expect.burned), `${c.id}`);
    } else {
      const [p, size, t, d, floor] = [c.parent_base_fee, c.parent_size, c.target, c.denominator, c.floor].map(BigInt);
      let next: bigint;
      if (t === 0n || d === 0n) next = p > floor ? p : floor;
      else {
        const gap = size > t ? size - t : t - size;
        const delta = (p * gap) / t / d;
        next = size > t ? p + (delta > 1n ? delta : 1n) : p > delta ? p - delta : 0n;
        if (next > U64_MAX) next = U64_MAX;
        if (next < floor) next = floor;
      }
      check(next === BigInt(c.expect), `${c.id}: got ${next}`);
    }
  }
  return doc.cases.length;
}

// ------------------------------------------------------------------ headers
function headers(): number {
  const doc = load("headers.json");
  for (const c of doc.cases) {
    const h = c.header;
    const bytes = concat(fromHex(h.prev_hash), fromHex(h.state_root), le64(BigInt(h.timestamp)), le64(BigInt(h.nonce)), fromHex(h.difficulty_target), fromHex(h.tx_root)); // CON-1
    if (!c.transactions) {
      check(toHex(bytes) === c.expect.bytes, `${c.id}: bytes`);
      check(toHex(deriveKey("custom-l1-node header id v1", bytes)) === c.expect.id, `${c.id}: id`); // CON-2
      continue;
    }
    let level = c.transactions.map((t: string) => {
      const f = decode(fromHex(t));
      if (!f) throw new Error(`${c.id}: undecodable transaction`);
      return deriveKey("custom-l1-node tx leaf v1", txid(concat(f.io, f.pk, le64(f.nonce)), f.sig)); // CON-3
    });
    while (level.length > 1) {
      const next: Uint8Array[] = [];
      for (let i = 0; i < level.length; i += 2) next.push(i + 1 < level.length ? node(level[i], level[i + 1]) : level[i]);
      level = next;
    }
    const matches = toHex(level[0]) === h.tx_root;
    check(matches === (c.expect.result === "ok"), `${c.id}: tx_root ${matches ? "matched" : "did not match"}`);
  }
  return doc.cases.length;
}

// -------------------------------------------------------------------- main
let failures = 0;
function check(ok: boolean, what: string): void {
  if (!ok) {
    failures++;
    console.error(`MISMATCH ${what}`);
  }
}

selftest();
const keyDoc = load("keys.json");
const keys = new Map<string, string>();
for (const k of keyDoc.keys) {
  const derived = toHex(blake3(concat(text("custom-l1-node.address.v3"), fromHex(k.public_key))));
  check(derived === k.address, `keys.json ${k.name}: address`);
  keys.set(k.name, k.address);
}
const counts = { state_transitions: stateTransitions(keys), encoding: encoding(), fees: fees(), headers: headers(), keys: keys.size };
console.log(`verifier-ts (independent, signatures trusted from vectors): ${JSON.stringify(counts)} cases, ${failures} mismatches`);
process.exit(failures === 0 ? 0 : 1);
