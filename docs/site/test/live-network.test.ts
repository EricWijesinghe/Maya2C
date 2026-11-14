// node --test test/  (Node 22.18+ strips the types itself; no build step)
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  MAX_BLOCKS, STALL_SECS, layoutRing, mapBlock, mapBlocks, mapConsensus, mapFinality, mapLive,
  mapStakeValidators, mergeBlocks, networkState, timeline,
} from "../src/scripts/live-network.ts";

// Shapes copied from apps/status/src/live.rs (LiveView), apps/explorer/src/model.rs
// (IndexedBlock) and crates/node/src/rpc/{types,validators}.rs.
const block = (height: number, timestamp: number, tx_count = 0) => ({
  height, id: `id${height}`, prev_hash: "00", state_root: "00", timestamp, nonce: 0, difficulty_target: "ff", tx_count, work: "1",
});

test("mapLive keeps each watched node's own answer", () => {
  const v = mapLive({
    at_ms: 1, network: "maya-testnet-1", peers: [], events: [], history: [],
    nodes: [
      { name: "v1", up: true, voting: true, height: 120, lag_rounds: 0, peers: 3, bft: { epoch: 4, round: 9, committed_round: 8, validator: 0, committee: 4, follower: false } },
      { name: "v2", up: false, voting: false, height: null, lag_rounds: null, peers: 0, bft: null },
      { name: "v3", up: true, voting: false, height: 119, lag_rounds: 2, peers: 1, bft: { follower: true } },
    ],
  });
  assert.deepEqual(v?.map((x) => [x.name, x.up, x.voting, x.follower, x.height, x.lagRounds]), [
    ["v1", true, true, false, 120, 0],
    ["v2", false, false, false, null, null],
    ["v3", true, false, true, 119, 2],
  ]);
});

test("mapLive turns malformed fields into null, never a guess", () => {
  const v = mapLive({ nodes: [{ name: "x", up: "yes", voting: 1, height: -5, lag_rounds: 1.5 }, { up: true }, null] });
  assert.equal(v?.length, 1, "a node without a name is dropped");
  assert.deepEqual(v?.[0], { name: "x", up: false, voting: false, follower: false, height: null, lagRounds: null, source: "status" });
  assert.equal(mapLive(null), null);
  assert.equal(mapLive({ nodes: "v1" }), null);
});

test("mapStakeValidators lists the committee only, with liveness unknown", () => {
  const s = mapStakeValidators({
    epoch: 7, epoch_blocks: 100, committee: 2, total: 3,
    validators: [
      { id: "aa".repeat(32), operator: "o", self_bond: "1", delegated: "0", commission_bps: 0, status: "bonded", jailed_until: null, in_committee: true },
      { id: "bb".repeat(32), operator: "o", self_bond: "1", delegated: "0", commission_bps: 0, status: "jailed", jailed_until: 9, in_committee: true },
      { id: "cc".repeat(32), operator: "o", self_bond: "1", delegated: "0", commission_bps: 0, status: "bonded", jailed_until: null, in_committee: false },
    ],
  });
  assert.equal(s?.epoch, 7);
  assert.equal(s?.committee, 2);
  assert.deepEqual(s?.validators.map((v) => [v.name, v.up, v.voting, v.source]), [
    ["aaaaaaaaaa", null, true, "stake"],
    ["bbbbbbbbbb", null, false, "stake"],
  ]);
  assert.equal(mapStakeValidators(null), null, "null on a chain without staking");
});

test("mapBlock rejects a block with any malformed field", () => {
  assert.deepEqual(mapBlock(block(5, 1000, 2)), { height: 5, time: 1000, txs: 2, id: "id5" });
  assert.equal(mapBlock({ ...block(5, 1000), height: "5" }), null);
  assert.equal(mapBlock({ ...block(5, 1000), timestamp: null }), null);
  assert.equal(mapBlock("block"), null);
});

test("mapBlocks sorts newest first, dedupes and caps", () => {
  const list = mapBlocks([block(1, 10), block(3, 30), block(2, 20), block(3, 31), "junk"]);
  assert.deepEqual(list?.map((b) => [b.height, b.time]), [[3, 31], [2, 20], [1, 10]]);
  const many = mapBlocks(Array.from({ length: MAX_BLOCKS + 10 }, (_, i) => block(i, i)));
  assert.equal(many?.length, MAX_BLOCKS);
  assert.equal(many?.[0].height, MAX_BLOCKS + 9);
  assert.equal(mapBlocks({}), null);
});

test("mergeBlocks does not mutate its input", () => {
  const before = [mapBlock(block(1, 10))!];
  const after = mergeBlocks(before, [mapBlock(block(2, 20))!]);
  assert.equal(before.length, 1);
  assert.deepEqual(after.map((b) => b.height), [2, 1]);
});

test("mapConsensus and mapFinality read the RPC results", () => {
  assert.deepEqual(mapConsensus({ epoch: 4, round: 9, committed_round: 8, validator: null, committee: 4, follower: false }),
    { epoch: 4, committee: 4, round: 9, committedRound: 8 });
  assert.equal(mapConsensus(null), null, "null on a node that runs no DAG-BFT");
  assert.deepEqual(mapFinality({ epoch: 4, height: 118, block: "ab", signatures: [[0, "s"], [1, "s"], [2, "s"]] }),
    { height: 118, epoch: 4, signatures: 3 });
  assert.equal(mapFinality({ epoch: 4, height: 118, block: "ab" }), null);
  assert.equal(mapFinality(null), null);
});

test("networkState never reads live without a fresh tip", () => {
  assert.equal(networkState(false, 1000, 1000), "offline");
  assert.equal(networkState(true, null, 1000), "partial");
  assert.equal(networkState(true, 1000, 1000 + STALL_SECS), "live");
  assert.equal(networkState(true, 1000, 1000 + STALL_SECS + 1), "stalled");
});

test("layoutRing keeps every node on the stage, front larger than back", () => {
  const pts = layoutRing(7);
  assert.equal(pts.length, 7);
  for (const p of pts) {
    assert.ok(p.x > 0 && p.x < 1 && p.y > 0 && p.y < 1, `inside the stage: ${JSON.stringify(p)}`);
  }
  assert.ok(pts[0].scale > Math.max(...pts.slice(1).map((p) => p.scale)) - 1e-9, "node 0 sits at the front");
  assert.deepEqual(layoutRing(0), []);
});

test("timeline spreads blocks by their own timestamps", () => {
  const t = timeline(mapBlocks([block(3, 40), block(2, 20), block(1, 0)])!);
  assert.deepEqual(t, [{ height: 3, at: 1 }, { height: 2, at: 0.5 }, { height: 1, at: 0 }]);
  assert.deepEqual(timeline([]), []);
  assert.deepEqual(timeline(mapBlocks([block(1, 5)])!), [{ height: 1, at: 1 }]);
});
