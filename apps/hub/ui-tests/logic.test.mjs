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

test("a built entry says paused when the node is unreachable, and the legend carries the date", () => {
  const built = UNIVERSE.filter((i) => i.status === "built");
  assert.ok(built.length > 0, "the wallet, explorer and validators are built");
  for (const item of built) {
    const chip = NetStatus.chip(item, "paused");
    assert.equal(chip.tone, "paused");
    assert.match(chip.text, /paused/i);
    // Fits one line of a two-column card at 360 px (11 px type).
    assert.ok(chip.text.length <= 24, `card chip too long for a phone: "${chip.text}"`);
  }
  assert.equal(NetStatus.legend("paused")[0].text, "Built, testnet paused since 2026-10-28");
});

test("the legend and the cards say live in the same words", () => {
  const wallet = UNIVERSE.find((u) => u.id === "wallet");
  for (const chainId of [null, "maya-testnet-1"]) {
    assert.equal(NetStatus.legend("live", chainId)[0].text, NetStatus.chip(wallet, "live", chainId).text, `chainId ${chainId}`);
  }
});

test("the paused copy sets no restart date and promises no genesis", () => {
  const copy = NetStatus.pausedCopy({ rpc: NetStatus.PUBLIC_RPC, error: "x" });
  assert.doesNotMatch(copy.body, /next genesis|opens again|soon|will (re)?start/i, copy.body);
  assert.match(copy.body, /no restart date/i);
});

test("an entry that is not built says so in its own sentence", () => {
  const lead = { building: /^Being built:/, planned: /^Planned:/, research: /^Research:/ };
  for (const item of UNIVERSE.filter((u) => u.status !== "built")) {
    assert.match(item.text, lead[item.status] || /^Planned:/, `${item.id}: "${item.text}"`);
  }
  for (const item of UNIVERSE) assert.doesNotMatch(item.text, /anyone can/i, `${item.id} claims open access`);
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

// ------------------------------------------------------------ DOM level ----
// The shipped scripts against a small element model: enough DOM for the
// first-start sheet and the rig cards, so these run in CI under plain node.
// ui-tests/browser-check.mjs runs the same flows in Chromium (CI job hub-ui).

class El {
  constructor(tag) {
    Object.assign(this, { tagName: tag.toUpperCase(), children: [], attrs: {}, on: {}, dataset: {}, style: {} });
    Object.assign(this, { hidden: false, disabled: false, checked: false, open: false, textContent: "", className: "", value: "" });
    this.classList = { toggle() {}, add() {}, remove() {}, contains: () => false };
  }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; }
  removeAttribute(k) { delete this.attrs[k]; }
  append(...kids) { for (const k of kids) (typeof k === "object" ? this.children.push(k) : (this.textContent += k)); }
  replaceChildren(...kids) { this.children = []; this.append(...kids); }
  addEventListener(type, fn) { (this.on[type] ||= []).push(fn); }
  fire(type) { (this.on[type] || []).forEach((fn) => fn({ target: this, key: "", stopPropagation() {} })); }
  *walk() { for (const c of this.children) { yield c; yield* c.walk(); } }
  querySelectorAll(sel) {
    const all = [...this.walk()];
    if (sel === "input:checked") return all.filter((e) => e.tagName === "INPUT" && e.checked);
    if (sel === "canvas") return all.filter((e) => e.tagName === "CANVAS");
    return [];
  }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  focus() {}
  getContext() { return {}; }
  showModal() { this.open = true; }
  close() { this.open = false; }
}

function dom() {
  const byId = new Map();
  const store = new Map();
  const document = {
    getElementById: (id) => byId.get(id) || byId.set(id, new El("div")).get(id),
    createElement: (tag) => new El(tag),
    createElementNS: (_ns, tag) => new El(tag),
    querySelectorAll: () => [],
    querySelector: () => null,
  };
  const localStorage = { getItem: (k) => (store.has(k) ? store.get(k) : null), setItem: (k, v) => store.set(k, String(v)) };
  return { document, localStorage, store };
}

test("first launch: no flag, the sheet opens, and Start runs only after the backup box", () => {
  const { document, localStorage, store } = dom();
  const realm = vm.createContext({ document, localStorage, navigator: {} });
  vm.runInContext(ui("status.js"), realm);
  vm.runInContext(ui("firstrun.js"), realm);
  const FirstRun = vm.runInContext("FirstRun", realm);
  const started = [];
  FirstRun.init({
    call: async () => undefined, isPreview: true, net: () => "live", label: (k) => k,
    settings: () => ({ key_file: "miner.key", rpc: NetStatus.PUBLIC_RPC }), gpus: () => [{ index: 0 }],
    onFinish: (devices) => started.push(devices),
  });
  const $ = (id) => document.getElementById(id);

  assert.equal(FirstRun.done(), false, "no flag stored: the sheet is due");
  FirstRun.open();
  assert.equal($("first-run").open, true, "the sheet opened");
  assert.equal($("fr-start").disabled, true, "Start is disabled before the backup box");
  $("fr-start").fire("click");
  assert.deepEqual(started, [], "a click on the disabled Start starts nothing");
  assert.equal(store.has("hub.firstRunDone"), false);

  $("fr-backup").checked = true;
  $("fr-backup").fire("change");
  assert.equal($("fr-start").disabled, false, "the backup box enables Start");
  $("fr-start").fire("click");
  assert.deepEqual(started.map((d) => [...d]), [["gpu0"]], "finishing starts the picked device");
  assert.equal(store.get("hub.firstRunDone"), "1", "the flag is stored");
  assert.equal($("first-run").open, false);
  assert.equal(FirstRun.done(), true);
});

test("every chart canvas has role=img and a non-empty label, or is hidden from assistive tech", () => {
  // The page's own canvases.
  const html = ui("index.html");
  const tags = html.match(/<canvas\b[^>]*>/g) || [];
  assert.ok(tags.length >= 4, "gauge, fx, rate chart, orb, block chart");
  for (const tag of tags) {
    const hidden = /aria-hidden="true"/.test(tag);
    const named = /role="img"/.test(tag) && /aria-label="[^"]*\S[^"]*"/.test(tag);
    assert.ok(hidden || named, tag);
  }
  // The canvases each rig card creates, named before their first draw.
  const { document } = dom();
  const realm = vm.createContext({ document, getComputedStyle: () => ({ getPropertyValue: () => "" }), window: {} });
  vm.runInContext(ui("charts.js"), realm);
  vm.runInContext(ui("rigs.js"), realm);
  realm.icon = () => new El("svg");
  const card = vm.runInContext('rigCard("gpu0")', realm);
  const canvases = card.querySelectorAll("canvas");
  assert.equal(canvases.length, 5, "a sparkline and four rings");
  for (const c of canvases) {
    assert.equal(c.getAttribute("role"), "img");
    assert.match(c.getAttribute("aria-label") || "", /\S/, "named before the first draw");
  }
});
