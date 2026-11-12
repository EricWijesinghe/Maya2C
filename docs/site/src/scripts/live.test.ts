// Run: node --test src/scripts/live.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import { blocksWithin, coins, hashShare, shareBp } from "./live.ts";

test("supply share is exact on 256-bit strings", () => {
  const total = (21_000_000n * 10n ** 24n).toString();
  assert.equal(shareBp((10_500_000n * 10n ** 24n).toString(), total), 5000);
  assert.equal(shareBp("1", total), 0);
  assert.equal(shareBp(total, total), 10000);
  assert.equal(shareBp("5", "0"), null, "no total: no ring, not a 100% one");
  assert.equal(shareBp("1.5", total), null, "a float is refused, never rounded");
});

test("hash share counts the visitor inside the network", () => {
  assert.equal(hashShare(100, 900), 0.1);
  assert.equal(hashShare(100, 0), null, "no measured network (chain paused): no share, never 100%");
  assert.equal(hashShare(0, 900), null);
  assert.equal(hashShare(-5, 900), null);
  assert.equal(hashShare(Number.NaN, 900), null);
  assert.equal(hashShare(100, Number.POSITIVE_INFINITY), null);
});

test("blocks in the last minute use header time, newest block as now", () => {
  const b = (height: number, timestamp: number) => ({ height, timestamp, tx_count: 0 });
  assert.equal(blocksWithin([b(1, 0), b(2, 30), b(3, 59), b(4, 61)], 60), 3);
  assert.equal(blocksWithin([], 60), 0);
});

test("whole coins round down and group", () => {
  assert.equal(coins((1_234_567n * 10n ** 24n + 999n).toString()), "1,234,567");
  assert.equal(coins("12"), "0");
  assert.equal(coins("-1"), null);
});
