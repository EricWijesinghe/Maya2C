// Run: node --test src/data/facts.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { homeFacts, type Health, type Progress } from "./facts.ts";

const load = (p: string) => JSON.parse(readFileSync(new URL(p, import.meta.url), "utf8"));
const health: Health = load("../../public/health.json");
const progress: Progress = load("../../public/progress.json");

test("every facts-band figure is the data file's own number", () => {
  const [ledger, gates, tests] = homeFacts(health, progress);
  assert.equal(ledger.value, `${health.ledger.by_status.verified} / ${health.ledger.total}`);
  assert.equal(gates.value, `${progress.gates.met} / ${progress.gates.total}`);
  assert.equal(tests.value, health.tests.passed.toLocaleString("en-US"));
  assert.match(tests.source, new RegExp(health.tests.as_of));
});

test("a failing test count is shown, not hidden", () => {
  const h = { ...health, tests: { ...health.tests, failed: 3 } };
  assert.match(homeFacts(h, progress)[2].label, /3 failing/);
});

test("no release from GitHub reads unknown, never a stale tag", () => {
  const f = homeFacts(health, { ...progress, latest_release: null })[3];
  assert.equal(f.value, "unknown");
});
