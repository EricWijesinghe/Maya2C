// Pure data mapping for the Live network page (src/pages/network.astro).
//
// Every input is untrusted JSON from a public endpoint. A field of the wrong
// type becomes null and the page shows it as MISSING; nothing here guesses,
// interpolates or carries an old value forward. Kept free of DOM and of
// non-erasable TypeScript so `node --test` runs it as-is (test/live-network.test.ts).

/** One validator as the page draws it. `up` is null when nobody watches it (stake record only). */
export type Validator = {
  name: string;
  up: boolean | null;
  voting: boolean;
  follower: boolean;
  height: number | null;
  lagRounds: number | null;
  source: "status" | "stake";
};
export type Block = { height: number; time: number; txs: number; id: string };
export type Consensus = { epoch: number | null; committee: number | null; round: number | null; committedRound: number | null };
export type Finality = { height: number; epoch: number; signatures: number };
export type NetState = "live" | "stalled" | "partial" | "offline";
export type Point = { x: number; y: number; scale: number };

/** Drawing limits, not protocol values: more would not fit the ring or the strip. */
export const MAX_VALIDATORS = 64;
export const MAX_BLOCKS = 24;
/** Display threshold, not a protocol value: a reachable chain whose tip is older reads "stalled". */
export const STALL_SECS = 300;

type Obj = Record<string, unknown>;
const obj = (v: unknown): Obj | null => (v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Obj) : null);
const uint = (v: unknown): number | null => (typeof v === "number" && Number.isSafeInteger(v) && v >= 0 ? v : null);
const text = (v: unknown, max = 64): string | null => (typeof v === "string" && v.length > 0 ? v.slice(0, max) : null);

/** Status service `/api/live`: the watched nodes, each with its own answer. */
export function mapLive(json: unknown): Validator[] | null {
  const nodes = obj(json)?.nodes;
  if (!Array.isArray(nodes)) return null;
  const out: Validator[] = [];
  for (const n of nodes.slice(0, MAX_VALIDATORS)) {
    const o = obj(n);
    const name = text(o?.name, 32);
    if (!o || !name) continue;
    out.push({
      name,
      up: o.up === true,
      voting: o.voting === true,
      follower: obj(o.bft)?.follower === true,
      height: uint(o.height),
      lagRounds: uint(o.lag_rounds),
      source: "status",
    });
  }
  return out;
}

/** Explorer `/api/validators` (the node's `get_validators`): the committee from the stake record. Liveness unknown. */
export function mapStakeValidators(json: unknown): { validators: Validator[]; epoch: number | null; committee: number | null } | null {
  const o = obj(json);
  if (!o || !Array.isArray(o.validators)) return null;
  const validators: Validator[] = [];
  for (const v of o.validators.slice(0, MAX_VALIDATORS)) {
    const r = obj(v);
    const id = text(r?.id, 128);
    if (!r || !id || r.in_committee !== true) continue;
    validators.push({ name: id.slice(0, 10), up: null, voting: r.status === "bonded", follower: false, height: null, lagRounds: null, source: "stake" });
  }
  return { validators, epoch: uint(o.epoch), committee: uint(o.committee) };
}

/** One explorer block (`/api/blocks` entry or a `/ws/blocks` frame); null if any field is malformed. */
export function mapBlock(json: unknown): Block | null {
  const o = obj(json);
  const height = uint(o?.height);
  const time = uint(o?.timestamp);
  const txs = uint(o?.tx_count);
  const id = text(o?.id, 128);
  if (height === null || time === null || txs === null || id === null) return null;
  return { height, time, txs, id };
}

/** Newest first, one per height, at most MAX_BLOCKS. */
export function mapBlocks(json: unknown): Block[] | null {
  if (!Array.isArray(json)) return null;
  return mergeBlocks([], json.map(mapBlock).filter((b): b is Block => b !== null));
}

/** Adds blocks to a list without mutating it; a later copy of a height replaces the earlier one. */
export function mergeBlocks(list: Block[], add: Block[]): Block[] {
  const byHeight = new Map<number, Block>();
  for (const b of [...list, ...add]) byHeight.set(b.height, b);
  return [...byHeight.values()].sort((a, b) => b.height - a.height).slice(0, MAX_BLOCKS);
}

/** `get_bft_status` result. */
export function mapConsensus(result: unknown): Consensus | null {
  const o = obj(result);
  if (!o) return null;
  return { epoch: uint(o.epoch), committee: uint(o.committee), round: uint(o.round), committedRound: uint(o.committed_round) };
}

/** `get_checkpoint` result: the newest quorum-attested block (ADR-038). */
export function mapFinality(result: unknown): Finality | null {
  const o = obj(result);
  const height = uint(o?.height);
  const epoch = uint(o?.epoch);
  if (!o || height === null || epoch === null || !Array.isArray(o.signatures)) return null;
  return { height, epoch, signatures: o.signatures.length };
}

/** What the banner says: reached nothing, reached something but no blocks, an old tip, or live. */
export function networkState(reachedAny: boolean, tipTime: number | null, nowSec: number): NetState {
  if (!reachedAny) return "offline";
  if (tipTime === null) return "partial";
  return nowSec - tipTime > STALL_SECS ? "stalled" : "live";
}

/** Validators on a tilted ring (2.5D): fractions of the stage, front of the ring larger. */
export function layoutRing(n: number): Point[] {
  return Array.from({ length: n }, (_, i) => {
    const a = Math.PI / 2 + (2 * Math.PI * i) / Math.max(1, n);
    const depth = (Math.sin(a) + 1) / 2; // 1 at the front (bottom), 0 at the back
    return { x: 0.5 + 0.4 * Math.cos(a), y: 0.5 + 0.34 * Math.sin(a), scale: 0.78 + 0.34 * depth };
  });
}

/** Each block's place on the timeline strip, 0 (oldest shown) to 1 (newest). */
export function timeline(blocks: Block[]): { height: number; at: number }[] {
  if (blocks.length === 0) return [];
  const times = blocks.map((b) => b.time);
  const lo = Math.min(...times);
  const span = Math.max(...times) - lo;
  return blocks.map((b) => ({ height: b.height, at: span === 0 ? 1 : (b.time - lo) / span }));
}
