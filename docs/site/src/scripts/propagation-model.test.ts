// Run: node --test src/scripts/propagation-model.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildModel, phaseAt, finalityAt, nodeLevel, PERIOD, NODE_COUNT, PHASES } from "./propagation-model.ts";

const m = buildModel();

test("graph is connected: every node is reached from the birth core", () => {
  assert.equal(m.points.length, NODE_COUNT);
  assert.ok(m.ignite.every((t) => Number.isFinite(t)), "a node was never reached");
});

test("quorum closes inside the cycle, with time left for the final pulse", () => {
  assert.equal(m.quorum, Math.ceil((m.validators.length * 2) / 3));
  assert.ok(m.finalAt + 2 < PERIOD, `finalAt ${m.finalAt} leaves no room in PERIOD ${PERIOD}`);
});

test("phases run in the caption order, once each", () => {
  const seen: number[] = [];
  for (let t = 0; t < PERIOD; t += 0.05) {
    const p = phaseAt(m, t);
    if (seen.at(-1) !== p) seen.push(p);
  }
  assert.deepEqual(seen, [0, 1, 2, 3]);
  assert.equal(PHASES.length, 4);
});

test("the core is lit before the front leaves it", () => {
  assert.ok(nodeLevel(m, m.core, 0.6) > 0);
  assert.equal(nodeLevel(m, m.core, 0), 0);
});

test("a far node stays dark until the front reaches it", () => {
  const far = m.ignite.indexOf(Math.max(...m.ignite));
  assert.equal(nodeLevel(m, far, m.ignite[far] - 0.01), 0);
});

test("finality is zero before the quorum and decays after it", () => {
  assert.equal(finalityAt(m, m.finalAt - 0.1).flash, 0);
  assert.ok(finalityAt(m, m.finalAt + 0.1).flash > finalityAt(m, m.finalAt + 1).flash);
});
