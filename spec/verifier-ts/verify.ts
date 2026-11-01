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
// ADR-046: balances and output amounts are 256 bits; nonces stay 64.
const U256_MAX = (1n << 256n) - 1n;
const load = (name: string) => JSON.parse(readFileSync(join(TESTS, name), "utf8"));
const leN = (v: bigint, n: number) => Uint8Array.from({ length: n }, (_, i) => Number((v >> BigInt(8 * i)) & 0xffn));
const le64 = (v: bigint) => leN(v, 8);
const le256 = (v: bigint) => leN(v, 32);
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

const leaf = (addr: string, a: Account) => blake3(concat(Uint8Array.of(0), fromHex(addr), le256(a.balance), le64(a.nonce)));
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
  for (const [, amount] of tx.outputs) if ((total += amount) > U256_MAX) return "BalanceOverflow";
  if (sender.balance < total) return "InsufficientBalance";
  if (sender.nonce === U64_MAX) return "BalanceOverflow";
  s.set(tx.from, { balance: sender.balance - total, nonce: sender.nonce + 1n });
  for (const [to, amount] of tx.outputs) {
    const r = s.get(to) ?? { balance: 0n, nonce: 0n };
    if (r.balance + amount > U256_MAX) return "BalanceOverflow";
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
  if (count(36) === null || count(64) === null) return null; // output: amount_le256 ‖ recipient
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
      const ceiling = BigInt(c.ceiling); // FEE-6 (ADR-071); the vectors carry it
      let next: bigint;
      if (t === 0n || d === 0n) next = p > floor ? p : floor;
      else {
        const gap = size > t ? size - t : t - size;
        const delta = (p * gap) / t / d;
        if (size > t) {
          const share = p / d > 1n ? p / d : 1n; // FEE-3: one denominator's share
          const rise = delta > 1n ? delta : 1n;
          next = p + (rise < share ? rise : share);
          const cap = ceiling > p ? ceiling : p; // FEE-6: a fee above the ceiling holds
          if (next > cap) next = cap;
          if (next > U256_MAX) next = U256_MAX;
        } else next = p > delta ? p - delta : 0n;
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
    const bytes = concat(fromHex(h.prev_hash), fromHex(h.state_root), le64(BigInt(h.timestamp)), le64(BigInt(h.nonce)), fromHex(h.difficulty_target), fromHex(h.tx_root), new Uint8Array([h.version])); // CON-1
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
// ------------------------------------------------------------------ mining
// MINE-1..10 from spec/09-mining.md, in BigInt: no width to overflow, so the
// 2^192 ceiling is checked on the exact sum and every product stays exact.
const BPS = 10000n;
const RETARGET_FACTOR = 4n;
const CEILING = 1n << 192n;
const bigOf = (v: unknown) => BigInt(v as string);

function epochReward(initial: bigint, tail: bigint, h: bigint, e: bigint): bigint {
  const n = e / h;
  const halved = n >= 256n ? 0n : initial >> n;
  return halved > tail ? halved : tail;
}

/** MINE-2's miners' part: `⌊reward × min(bps, 10000) / 10000⌋`. */
function bpsOf(value: bigint, bps: bigint): bigint {
  return (value * (bps < BPS ? bps : BPS)) / BPS;
}

/** MINE-4: `min(⌊budget / shares⌋, budget − paid)`, zero when shares is zero. */
function shareReward(reward: bigint, bps: bigint, shares: bigint, paid: bigint): bigint {
  if (shares === 0n) return 0n;
  const budget = bpsOf(reward, bps);
  const left = budget > paid ? budget - paid : 0n;
  const perShare = budget / shares;
  return perShare < left ? perShare : left;
}

/** MINE-3: the lifetime emission, or null at or past 2^192. */
function lifetime(initial: bigint, tail: bigint, h: bigint, blocks: bigint): bigint | null {
  const epochs = U64_MAX / blocks + 1n;
  let total = 0n;
  for (let start = 0n, k = 0n; start < epochs; start += h, k += 1n) {
    const halved = k >= 256n ? 0n : initial >> k;
    const left = epochs - start;
    if (halved <= tail) { total += tail * left; break; }
    total += halved * (h < left ? h : left);
  }
  return total < CEILING ? total : null;
}

/** MINE-5: the next share target, clamped to `limit` and never zero. */
function retarget(target: bigint, limit: bigint, shares: bigint, spent: bigint, blocks: bigint, ts: bigint): bigint {
  const scaled = spent === 0n ? shares : (shares * blocks) / spent;
  const demand = scaled > U64_MAX ? U64_MAX : scaled;
  const low = ts / RETARGET_FACTOR > 1n ? ts / RETARGET_FACTOR : 1n;
  const high = ts * RETARGET_FACTOR;
  const observed = demand < low ? low : demand > high ? high : demand;
  const next = (target * ts) / observed;
  if (next > limit) return limit;
  return next === 0n ? 1n : next;
}

/** MINE-6 to MINE-8 (and MINE-3): the first rule a schedule breaks, or null. */
function refusedBy(c: any): string | null {
  const [initial, tail, h, ts, blocks, mspb, bps, spent, target, limit] = [
    "initial", "tail", "halving_epochs", "target_shares", "epoch_blocks", "max_shares_per_block",
    "miner_share_bps", "spent_after", "target", "target_limit",
  ].map((k) => bigOf(c[k]));
  if (h === 0n || ts === 0n || blocks === 0n || mspb === 0n || bps > BPS || tail > initial || spent > blocks) return "MINE-6";
  if (bps > 0n) {
    const first = shareReward(initial, bps, ts, 0n);
    const tailShare = shareReward(tail, bps, ts, 0n);
    if (first === 0n || (tail !== 0n && tailShare === 0n)) return "MINE-7";
  }
  if (lifetime(initial, tail, h, blocks) === null) return "MINE-3";
  if (target === 0n || target > limit) return "MINE-8";
  return null;
}

function mining(): number {
  const doc = load("mining.json");
  for (const c of doc.cases) {
    if (c.fn === "epoch_reward") {
      const got = epochReward(bigOf(c.initial), bigOf(c.tail), bigOf(c.halving_epochs), bigOf(c.epoch));
      check(got === bigOf(c.expect), `${c.id}: got ${got}`);
    } else if (c.fn === "split") {
      const r = bigOf(c.reward), bps = bigOf(c.miner_bps) < BPS ? bigOf(c.miner_bps) : BPS;
      const miners = (r * bps) / BPS;
      check(miners === bigOf(c.expect.miners) && r - miners === bigOf(c.expect.committee), `${c.id}`);
    } else if (c.fn === "lifetime_emission") {
      const total = lifetime(bigOf(c.initial), bigOf(c.tail), bigOf(c.halving_epochs), bigOf(c.epoch_blocks));
      const want = c.expect.result === "ok" ? bigOf(c.expect.total) : null;
      check(total === want, `${c.id}: got ${total}`);
    } else if (c.fn === "share_reward") {
      const got = shareReward(bigOf(c.reward), bigOf(c.miner_bps), bigOf(c.target_shares), bigOf(c.paid));
      check(got === bigOf(c.expect), `${c.id}: got ${got}`);
    } else if (c.fn === "retarget") {
      const got = retarget(bigOf(c.target), bigOf(c.target_limit), bigOf(c.shares), bigOf(c.spent_after), bigOf(c.epoch_blocks), bigOf(c.target_shares));
      check(got === bigOf(c.expect), `${c.id}: got ${got}`);
    } else if (c.fn === "validate") {
      const got = refusedBy(c);
      const want = c.expect.result === "ok" ? null : c.expect.error;
      check(got === want, `${c.id}: refused by ${got}`);
    } else if (c.fn === "share_header") {
      const material = concat(fromHex(c.chain_tag), le64(bigOf(c.epoch)), fromHex(c.anchor), fromHex(c.miner));
      check(toHex(deriveKey("maya2c mining share v1", material)) === c.expect, `${c.id}: share header`);
    } else if (c.fn === "meets_target") {
      const met = bigOf("0x" + c.digest) <= bigOf("0x" + c.target);
      check(met === (c.expect.result === "ok"), `${c.id}: meets target`);
    } else check(false, `${c.id}: unknown fn ${c.fn}`);
  }
  return doc.cases.length;
}

const counts = { state_transitions: stateTransitions(keys), encoding: encoding(), fees: fees(), headers: headers(), mining: mining(), keys: keys.size };
console.log(`verifier-ts (independent, signatures trusted from vectors): ${JSON.stringify(counts)} cases, ${failures} mismatches`);
process.exit(failures === 0 ? 0 : 1);
