// Run: node --test src/data/home.test.ts (from docs/site)
import { test } from "node:test";
import assert from "node:assert/strict";
import { heroCtas, uptimePercent, unreachableLabel, FAILS_BEFORE_UNREACHABLE, runDays, finalityScaleSeconds } from "./home.ts";

test("the uptime strip has one bar per calendar day the run touched", () => {
  const days = runDays("2026-10-20", "2026-10-28");
  assert.equal(days.length, 9);
  assert.equal(days[0], "2026-10-20");
  assert.equal(days.at(-1), "2026-10-28");
  assert.deepEqual(runDays("2026-10-31", "2026-11-01"), ["2026-10-31", "2026-11-01"]);
});

test("the finality scale ends past the figure, on a whole second", () => {
  assert.equal(finalityScaleSeconds(1545), 2);
  assert.equal(finalityScaleSeconds(2000), 3);
  assert.equal(finalityScaleSeconds(40), 1);
});

test("a paused testnet never offers mining as the first door", () => {
  const { primary, secondary } = heroCtas(true);
  assert.equal(primary.label, "See the 7-day run");
  assert.equal(primary.href, "/health/");
  assert.doesNotMatch(`${primary.label} ${secondary.label}`, /mining/i);
});

test("flipping the flag brings Start mining back", () => {
  assert.deepEqual(heroCtas(false).primary, { label: "Start mining", href: "/guides/start-mining/" });
  assert.deepEqual(heroCtas(false).secondary, heroCtas(true).secondary);
});

test("uptime is whole percent and never rounds up", () => {
  assert.equal(uptimePercent(1), "100%");
  assert.equal(uptimePercent(0.996), "99%");
  assert.equal(uptimePercent(0.29), "29%");
  assert.equal(uptimePercent(0), "0%");
});

test("an unreachable tile keeps its last value and dates it", () => {
  assert.equal(unreachableLabel(null), "unreachable");
  assert.equal(
    unreachableLabel({ value: "1,234", at: new Date("2026-11-14T09:05:30Z") }),
    "unreachable, last known 1,234 2026-11-14 09:05 UTC",
  );
  assert.equal(FAILS_BEFORE_UNREACHABLE, 2);
});
