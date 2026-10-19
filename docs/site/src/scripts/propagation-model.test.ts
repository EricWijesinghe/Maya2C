// Run: node --test src/scripts/propagation-model.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  buildModel, phaseAt, finalityAt, nodeLevel, liftAt, pushAt, frontRadius, cameraDistance,
  PERIOD, PHASES, FRONT_START,
} from "./propagation-model.ts";

const m = buildModel();

test("every node is reached by the wavefront, and nearer nodes light first", () => {
  assert.ok(m.ignite.every((t) => Number.isFinite(t)));
  const near = m.dist.indexOf(Math.min(...m.dist.filter((d) => d > 0)));
  const far = m.dist.indexOf(m.maxDist);
  assert.ok(m.ignite[near] < m.ignite[far]);
});

test("the quorum closes inside the cycle, with a hold before the camera eases out", () => {
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

test("a node stays dark until the wavefront reaches it, then lights", () => {
  const far = m.dist.indexOf(m.maxDist);
  assert.equal(nodeLevel(m, far, m.ignite[far] - 0.01), 0);
  assert.ok(nodeLevel(m, far, m.ignite[far] + 0.5) > 0.5);
});

test("the wavefront overshoots slightly and never exceeds about 4% of the field", () => {
  let peak = 0;
  for (let t = FRONT_START; t < FRONT_START + 4; t += 0.01) peak = Math.max(peak, frontRadius(m, t));
  assert.ok(peak > m.maxDist, "no overshoot at all");
  assert.ok(peak < m.maxDist * 1.05, `overshoot too large: ${peak / m.maxDist}`);
});

test("the lattice lifts under the shell and not far ahead of it", () => {
  const R = frontRadius(m, FRONT_START + 1.5);
  const under = m.dist.findIndex((d) => Math.abs(d - R) < 0.1);
  const farAhead = m.dist.indexOf(m.maxDist);
  assert.ok(liftAt(m, under, FRONT_START + 1.5) > 0.15);
  assert.ok(liftAt(m, farAhead, FRONT_START + 1.5) < liftAt(m, under, FRONT_START + 1.5) / 2);
});

test("the camera pushes in and returns to rest at the cycle boundary", () => {
  assert.equal(pushAt(m, 0), 0);
  assert.ok(Math.abs(pushAt(m, PERIOD - 1e-6)) < 1e-3);
  assert.ok(cameraDistance(m, m.finalAt) < cameraDistance(m, FRONT_START));
});

test("finality is zero before the quorum and decays after it", () => {
  assert.equal(finalityAt(m, m.finalAt - 0.1).flash, 0);
  assert.ok(finalityAt(m, m.finalAt + 0.1).flash > finalityAt(m, m.finalAt + 1).flash);
});
