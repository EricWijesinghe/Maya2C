// Hub UI logic without a browser: `node --test apps/hub/ui-tests/`.
// apps/hub/src-tauri/tests/ui_strings.rs runs this file in CI, so the rule
// "nothing is live while the node does not answer" fails a build, not a review.
//
// The scripts are loaded the way the webview loads them (classic scripts in one
// realm), so the production files carry no test hook.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const ui = (name) => readFileSync(fileURLToPath(new URL(`../ui/${name}`, import.meta.url)), "utf8");

function load() {
  const realm = vm.createContext({});
  vm.runInContext(ui("status.js"), realm);
  vm.runInContext(ui("universe.js"), realm);
  return vm.runInContext("({ NetStatus, UNIVERSE })", realm);
}

const { NetStatus, UNIVERSE } = load();
const UNREACHABLE = [null, { reachable: false, error: "connect refused" }, { reachable: "true" }, {}];

test("no universe entry is written as live: live is read from the node", () => {
  for (const item of UNIVERSE) assert.notEqual(item.status, "live", `${item.id} hard-codes live`);
});

test("no universe chip says live while the node does not answer", () => {
  for (const snap of UNREACHABLE) {
    const net = NetStatus.of(snap);
    assert.notEqual(net, "live", `snapshot ${JSON.stringify(snap)} read as live`);
    for (const item of UNIVERSE) {
      const chip = NetStatus.chip(item, net);
      assert.notEqual(chip.tone, "live", `${item.id} is live while unreachable`);
      assert.doesNotMatch(chip.text, /\blive\b/i, `${item.id}: "${chip.text}"`);
    }
  }
});

test("a built entry says paused, with the date, when the node is unreachable", () => {
  const built = UNIVERSE.filter((i) => i.status === "built");
  assert.ok(built.length > 0, "the wallet, explorer and validators are built");
  for (const item of built) {
    assert.equal(NetStatus.chip(item, "paused").text, "Built, testnet paused since 2026-10-28");
  }
});

test("only built entries turn live, and only on a reachable snapshot", () => {
  const net = NetStatus.of({ reachable: true, chain: { chain_id: "maya-testnet-1", height: 1 } });
  assert.equal(net, "live");
  for (const item of UNIVERSE) {
    const chip = NetStatus.chip(item, net);
    assert.equal(chip.tone === "live", item.status === "built", item.id);
  }
});

test("the legend never offers a live chip while paused", () => {
  for (const net of ["paused", "connecting"]) {
    for (const entry of NetStatus.legend(net)) assert.doesNotMatch(entry.text, /\blive\b/i, entry.text);
  }
  assert.ok(NetStatus.legend("live").some((e) => e.tone === "live"));
});

test("no entry dates a trial that did not happen", () => {
  for (const item of UNIVERSE) assert.doesNotMatch(item.text, /mid-October/i, item.id);
});

test("the paused card names the stop on the public node, and keeps raw errors in Details", () => {
  const raw = "rpc: error sending request for url (https://rpc.maya2c.dev/rpc): dns error";
  const pub = NetStatus.pausedCopy({ rpc: "https://rpc.maya2c.dev/rpc/", error: raw });
  assert.match(pub.title, /paused/i);
  assert.match(pub.body, /2026-10-28/);
  assert.ok(pub.report, "links the stop report");
  assert.ok(!pub.title.includes(raw) && !pub.body.includes(raw), "raw error stays out of the copy");
  assert.equal(pub.details, raw);
  assert.doesNotMatch(`${pub.title} ${pub.body}`, /unreachable/i);

  const own = NetStatus.pausedCopy({ rpc: "http://127.0.0.1:18545/rpc", error: "refused" });
  assert.doesNotMatch(own.body, /paused since/i, "a local node being down is not the testnet pause");
  assert.match(own.body, /127\.0\.0\.1:18545/);
  assert.equal(own.report, false);
});

test("Start is blocked, with a reason, unless the node answers and a device is picked", () => {
  const base = { canMine: true, devices: ["gpu0"], rpc: "https://rpc.maya2c.dev/rpc" };
  assert.equal(NetStatus.startBlock({ ...base, net: "live" }), null);
  assert.match(NetStatus.startBlock({ ...base, net: "paused" }), /paused since 2026-10-28/);
  assert.match(NetStatus.startBlock({ ...base, net: "connecting" }), /\S/);
  assert.match(NetStatus.startBlock({ ...base, net: "live", devices: [] }), /device/i);
  assert.match(NetStatus.startBlock({ ...base, net: "live", canMine: false }), /desktop/i);
});

test("the first-run sheet cannot start before the backup box is ticked", () => {
  assert.equal(NetStatus.sheetReady({ backup: false, devices: ["gpu0"] }), false);
  assert.equal(NetStatus.sheetReady({ backup: true, devices: [] }), false);
  assert.equal(NetStatus.sheetReady({ backup: true, devices: ["cpu"] }), true);
});
