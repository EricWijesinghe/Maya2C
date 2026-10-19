// Run: node --test src/components/math/math.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import { faults, quorum, legacyQuorum, minOverlap, isSafe } from "./quorum.ts";
import { gaussReduce, nearestLatticePoint, determinant, type Basis } from "./lattice.ts";
import { buildLevels, proofFor, rootFromProof } from "./merkle.ts";
import { log2Cost } from "./growth.ts";

test("quorum n - f is safe at every committee size from 1 to 1000 (ADR-039)", () => {
  for (let n = 1; n <= 1000; n++) {
    assert.ok(isSafe(n, quorum(n)), `n = ${n}`);
  }
});

test("the retired 2f + 1 rule fails where ADR-039 says it does", () => {
  assert.equal(legacyQuorum(4), 3);
  assert.equal(isSafe(4, legacyQuorum(4)), true);
  assert.equal(isSafe(5, legacyQuorum(5)), false); // overlap 1, need 2
  assert.equal(isSafe(6, legacyQuorum(6)), false); // two disjoint halves
  assert.equal(minOverlap(legacyQuorum(6), 6), 0);
});

test("n = 6: quorum 5, overlap 4, one fault", () => {
  assert.equal(faults(6), 1);
  assert.equal(quorum(6), 5);
  assert.equal(minOverlap(quorum(6), 6), 4);
});

test("gauss reduction keeps the lattice and exposes its short vectors", () => {
  const bad: Basis = [[9, 2], [4, 1]];
  const good = gaussReduce(bad);
  assert.equal(Math.abs(determinant(good)), Math.abs(determinant(bad)));
  const norms = good.map((v) => Math.hypot(v[0], v[1])).sort();
  assert.deepEqual(norms, [1, 1]);
});

test("nearest lattice point finds (1, 0) by brute force in the skewed basis", () => {
  const bad: Basis = [[9, 2], [4, 1]];
  const found = nearestLatticePoint(bad, [1, 0], 12);
  assert.deepEqual(found.point, [1, 0]);
  assert.equal(found.tries, 25 * 25);
});

test("merkle proofs have log2(n) steps and verify only the true leaf", async () => {
  const values = Array.from({ length: 8 }, (_, i) => `tx ${i}`);
  const levels = await buildLevels(values);
  const root = levels[levels.length - 1][0];
  for (let i = 0; i < values.length; i++) {
    const steps = proofFor(levels, i);
    assert.equal(steps.length, 3);
    assert.equal(await rootFromProof(values[i], steps), root);
  }
  assert.notEqual(await rootFromProof("tx 3 forged", proofFor(levels, 3)), root);
});

test("merkle rejects a leaf count that is not a power of two", async () => {
  await assert.rejects(buildLevels(["a", "b", "c"]), RangeError);
});

test("growth: Grover halves the exponent; Shor is polynomial", () => {
  assert.ok(Math.abs(log2Cost("grover", 256) - (128 + Math.log2(Math.PI / 4))) < 1e-9);
  assert.equal(log2Cost("brute", 128), 128);
  assert.ok(log2Cost("shor", 2048) < 40);
});
