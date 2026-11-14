// Maya2C Hub: polling, state and rendering. All live numbers come from the
// Rust commands in src-tauri; opened in a plain browser, the page runs on
// synthetic preview data and says so in the header. Whether the network is
// live is decided in one place, NetStatus.of(snapshot) (status.js).
"use strict";

const BASE_PER_MAYA = 1e24; // 24 decimals (ADR-068): 21M MAYA = 2.1e31 base units
const POLL_MINER_MS = 1000;
const POLL_NODE_MS = 4000;
const POLL_GPU_MS = 3000;
const HEIGHT_HISTORY = 300;
const PACE_MIN_MS = 5 * 60_000; // a rate from under five minutes of payouts is noise
const DEFAULT_CEILING_C = 80; // the Hub's default, mirrored from watchdog.rs
const FEED_ROWS = 30;
const ALERT_ROWS = 50;
const SVG_NS = "http://www.w3.org/2000/svg";

const tauri = window.__TAURI__ && window.__TAURI__.core;
const isPreview = !tauri;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const $ = (id) => document.getElementById(id);

const state = {
  settings: null,
  defaults: null,
  miner: null, // { workers: [MinerState], alerts: [Alert] }
  snapshot: null,
  net: "connecting", // NetStatus.of(snapshot)
  gpus: { devices: [], note: null },
  peak: 0,
  lastAccepted: 0,
  heights: [],
  lastHeight: null,
  rig: null, // device key of the rig the learn-more panel shows
  watchOnly: false,
  startError: null,
};

// ---------------------------------------------------------------- data ----

// The Rust commands, or preview.js when the page runs in a plain browser.
async function call(cmd, args) {
  if (isPreview) {
    if (preview[cmd]) return preview[cmd](args);
    throw new Error("preview mode cannot " + cmd);
  }
  return tauri.invoke(cmd, args);
}

function loadSettings(defaults) {
  try {
    const saved = JSON.parse(localStorage.getItem("hub.settings") || "null");
    return { ...defaults, ...(saved || {}) };
  } catch {
    return { ...defaults };
  }
}

function saveSettings(settings) {
  try {
    localStorage.setItem("hub.settings", JSON.stringify(settings));
    return true;
  } catch {
    return false;
  }
}

// ------------------------------------------------------------- format ----

const maya = (base) => (base == null ? "—" : (base / BASE_PER_MAYA).toLocaleString(undefined, { maximumFractionDigits: 4 }));
const uptime = (ms) => {
  if (!ms) return "—";
  const s = Math.floor((Date.now() - ms) / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h ? `${h}h ${m}m` : `${m}m ${s % 60}s`;
};
const clock = (t) => new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" });

function setText(id, value) {
  const el = typeof id === "string" ? $(id) : id;
  if (el && el.textContent !== String(value)) el.textContent = value;
}

function icon(name, cls = "icon") {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("class", cls);
  svg.setAttribute("aria-hidden", "true");
  const use = document.createElementNS(SVG_NS, "use");
  use.setAttribute("href", `icons.svg#${name}`);
  svg.append(use);
  return svg;
}

// A sentence, with the raw error under a Details disclosure when there is one.
function explain(host, sentence, raw) {
  const key = `${sentence}|${raw || ""}`;
  if (host.dataset.key === key) return;
  host.dataset.key = key;
  if (!sentence) return host.replaceChildren();
  const parts = [document.createTextNode(sentence)];
  if (raw) {
    const details = document.createElement("details");
    const summary = document.createElement("summary");
    const code = document.createElement("code");
    summary.textContent = "Details";
    code.textContent = String(raw);
    details.append(summary, code);
    parts.push(details);
  }
  host.replaceChildren(...parts);
}

const deviceLabel = (key, gpu) => (key === "cpu" ? "CPU threads" : `GPU ${key.slice(3)}${gpu ? ` · ${gpu.name}` : ""}`);
const gpuKey = (index) => `gpu${index}`;

// ------------------------------------------------------------- summary ----

const lastRate = (w) => (w.rates && w.rates.length ? w.rates[w.rates.length - 1].hps : 0);
const sumOf = (list, key) => list.reduce((total, w) => total + (w[key] || 0), 0);

// The hero adds the rigs up. Every rig pays the same account, so the received
// figure is the largest on-chain growth any rig has seen, not a sum.
function summarise(workers) {
  const running = workers.filter((w) => w.running);
  const shown = running[0] || workers.find((w) => w.status && w.status !== "stopped");
  const epochs = workers.map((w) => w.epoch).filter((e) => e != null);
  const rewards = workers.filter((w) => w.share_reward != null);
  const starts = running.map((w) => w.started_ms).filter(Boolean);
  const payee = workers.find((w) => w.address && w.address !== "preview");
  return {
    running: running.length > 0,
    status: running.length > 1 ? `${running.length} running · ${running[0].status}` : shown ? shown.status : "Stopped",
    hps: running.reduce((total, w) => total + lastRate(w), 0),
    accepted: sumOf(workers, "accepted"),
    rejected: sumOf(workers, "rejected"),
    received: workers.reduce((most, w) => Math.max(most, Number(w.earned) || 0), 0),
    epoch: epochs.length ? Math.max(...epochs) : null,
    reward: rewards.length ? rewards[0].share_reward : null,
    started_ms: starts.length ? Math.min(...starts) : null,
    address: payee ? payee.address : null,
  };
}

// The hero chart: every rig's samples added up, aligned on the newest sample.
function combinedRates(workers) {
  const series = workers.map((w) => w.rates || []).filter((r) => r.length);
  const longest = Math.max(0, ...series.map((r) => r.length));
  const out = [];
  for (let back = longest - 1; back >= 0; back--) {
    let hps = 0;
    let t_ms = 0;
    for (const r of series) {
      const sample = r[r.length - 1 - back];
      if (sample) {
        hps += sample.hps;
        t_ms = Math.max(t_ms, sample.t_ms);
      }
    }
    out.push({ t_ms, hps });
  }
  return out;
}

// Paid rate from what the chain actually credited, never from accepted shares
// (accepted is not paid: 145 accepted against 10 paid on the mining devnet).
function paceLine(sum) {
  const ran = sum.running && sum.started_ms ? Date.now() - sum.started_ms : 0;
  if (ran < PACE_MIN_MS) return sum.running ? "Rate shown after 5 min of payouts" : "";
  const perHour = (Number(sum.received) * 3_600_000) / ran;
  return `≈ ${maya(perHour)} MAYA/h received, measured`;
}

// An empty tile ("—") shows its one-line hint instead.
function hintTiles() {
  document.querySelectorAll(".tile .hint").forEach((hint) => {
    const value = hint.parentElement.querySelector("b");
    hint.hidden = !value || value.textContent !== "—";
  });
}

// ------------------------------------------------------------- render ----

// The saved picks, or the default: every GPU, or the CPU when there is none.
function chosenDevices() {
  if (state.settings.devices) return state.settings.devices.filter(Boolean);
  const gpus = state.gpus.devices.map((g) => gpuKey(g.index));
  return gpus.length ? gpus : ["cpu"];
}

function startReason() {
  return NetStatus.startBlock({ net: state.net, canMine: !state.watchOnly, devices: chosenDevices(), rpc: state.settings.rpc });
}

function renderPower(running) {
  const power = $("power");
  power.setAttribute("aria-pressed", String(running));
  setText("power-text", running ? "Stop mining" : "Start mining");
  // Stop always works; Start waits for the network and says why.
  const block = running ? null : startReason();
  power.disabled = Boolean(block);
  if (block) explain($("power-why"), block);
  else if (state.startError) explain($("power-why"), "The miner did not start.", state.startError);
  else explain($("power-why"), "");
}

function renderMiner() {
  const report = state.miner;
  if (!report) return;
  const sum = summarise(report.workers);
  if (sum.hps) state.peak = Math.max(state.peak * 0.999, sum.hps * 1.1);
  const r = Charts.rate(sum.running ? sum.hps : null);
  setText("rate-value", r.value);
  setText("rate-unit", r.unit);
  Charts.describe($("gauge"), sum.running ? `Hash rate gauge: ${r.value} ${r.unit}` : "Hash rate gauge: not mining");
  setText("miner-status", state.watchOnly ? WATCH_ONLY : sum.status);
  setText("t-accepted", sum.accepted.toLocaleString());
  setText("t-rejected", sum.rejected.toLocaleString());
  setText("t-received", maya(sum.received));
  setText("t-epoch", sum.epoch ?? "—");
  setText("t-reward", sum.reward == null ? "—" : maya(sum.reward));
  setText("t-uptime", sum.running ? uptime(sum.started_ms) : "—");
  setText("t-pace", paceLine(sum));
  renderPower(sum.running);
  // The node's snapshot carries the Bech32m form (ADR-081) and any refusal of it.
  const snap = state.snapshot || {};
  const payee = snap.address_error ? `Payout address refused: ${snap.address_error}`
    : sum.address ? `Paying to ${snap.address_text || sum.address}` : "";
  setText("payout-line", payee);

  if (sum.accepted > state.lastAccepted) {
    const fx = $("fx").getBoundingClientRect();
    if (!reducedMotion) Charts.burst(fx.width / 2, fx.height * 0.45);
    const tile = $("t-received").closest(".tile");
    tile.classList.remove("flash");
    void tile.offsetWidth;
    tile.classList.add("flash");
  }
  state.lastAccepted = sum.accepted;
  Charts.area($("rate-chart"), combinedRates(report.workers), { name: "Hash rate", empty: "No samples yet: start the miner" });
  renderFeed(report.workers);
  renderAlerts(report.alerts);
  renderRigs();
  hintTiles();
}

// The shared card shown on Mining and Network while the node does not answer.
function renderPaused() {
  const copy = state.net === "paused" ? NetStatus.pausedCopy({ rpc: state.settings.rpc, error: state.snapshot && state.snapshot.error }) : null;
  document.querySelectorAll("[data-paused]").forEach((slot) => {
    slot.hidden = !copy;
    if (!copy) return;
    if (!slot.firstElementChild) {
      slot.append($("paused-tpl").content.cloneNode(true));
      const link = slot.querySelector(".paused-report");
      link.href = NetStatus.STOP_REPORT;
      link.addEventListener("click", openReport);
    }
    setText(slot.querySelector(".paused-title"), copy.title);
    setText(slot.querySelector(".paused-text"), copy.body);
    slot.querySelector(".paused-report").hidden = !copy.report;
    slot.querySelector(".paused-details").hidden = !copy.details;
    setText(slot.querySelector(".paused-raw"), copy.details);
  });
}

// In the app the opener opens the compiled-in report URL; a preview follows the link.
function openReport(e) {
  if (isPreview) return;
  e.preventDefault();
  call("open_stop_report").catch(() => undefined);
}

function renderNode() {
  const s = state.snapshot;
  const net = NetStatus.of(s);
  const changed = net !== state.net;
  state.net = net;
  renderPaused();
  if (changed) updateUniverse();
  if (state.miner) renderPower(state.miner.workers.some((w) => w.running));
  FirstRun.refresh();
  const pill = $("net-pill");
  if (net !== "live") {
    pill.className = `chip net ${net === "paused" ? "paused" : "idle"}`;
    setText("net-text", net === "paused" ? (NetStatus.isPublic(state.settings.rpc) ? `Testnet paused since ${NetStatus.PAUSED_SINCE}` : "Node not answering") : "Connecting…");
    NetView.update(s, null, false);
    NetView.frame(performance.now());
    hintTiles();
    return;
  }
  const height = s.chain.height;
  pill.className = "chip net live";
  setText("net-text", `${s.chain.chain_id} · #${height.toLocaleString()}`);
  const newBlock = state.lastHeight != null && height > state.lastHeight;
  if (state.lastHeight !== height) {
    state.heights.push({ t_ms: Date.now(), hps: height });
    if (state.heights.length > HEIGHT_HISTORY) state.heights.shift();
  }
  state.lastHeight = height;

  setText("n-height", height.toLocaleString());
  setText("n-chain", s.chain.chain_id);
  setText("n-committee", s.bft ? s.bft.committee : "—");
  setText("n-fee", s.fees && s.fees.active ? s.fees.base_fee : "off");
  const work = s.mining;
  setText("n-mining", work ? (work.active ? `open · epoch ${work.epoch}` : "not open yet") : "—");
  let rate = null;
  if (state.heights.length > 1) {
    const a = state.heights[0];
    const b = state.heights[state.heights.length - 1];
    rate = ((b.hps - a.hps) / Math.max(1, b.t_ms - a.t_ms)) * 60000;
    setText("n-rate", rate.toFixed(1));
  }
  NetView.update(s, rate, newBlock);
  NetView.frame(performance.now());
  drawHeights();

  setText("t-balance", s.account ? maya(s.account.balance) : "—");
  if (work && work.active) {
    const pct = Math.min(100, (100 * work.shares) / Math.max(1, work.target_shares));
    setText("t-epoch-shares", `${work.shares} / ${work.target_shares} shares`);
    $("t-epoch-bar").style.width = `${pct}%`;
    Charts.describe($("t-epoch-meter"), `Epoch shares: ${work.shares} of ${work.target_shares}`);
  }
  hintTiles();
}

function drawHeights() {
  Charts.area($("height-chart"), state.heights, { floor: true, name: "Block height", empty: "No blocks seen yet",
    label: (v) => ({ value: Math.round(v).toLocaleString(), unit: "" }) });
}

function renderLegend() {
  $("legend").replaceChildren(
    ...NetStatus.legend(state.net).map((entry) => {
      const chip = document.createElement("span");
      chip.className = `chip ${entry.tone}`;
      const dot = document.createElement("i");
      dot.className = "dot";
      chip.append(dot, entry.text);
      return chip;
    }),
  );
}

function universeDetail(item) {
  const chip = NetStatus.chip(item, state.net, state.snapshot && state.snapshot.chain && state.snapshot.chain.chain_id);
  const stage = item.status !== "built" ? `Stage: ${chip.text}.`
    : state.net === "live" ? `${chip.text}: the node answered the last poll.`
    : state.net === "paused" ? `${chip.text}. It comes back with the next genesis.`
    : "Built. Checking whether the network answers.";
  return stage;
}

function renderUniverse() {
  const host = $("universe");
  host.replaceChildren(
    ...UNIVERSE.map((item) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "planet";
      b.dataset.id = item.id;
      const glyph = document.createElement("span");
      glyph.className = "glyph";
      glyph.append(icon(item.icon));
      const name = document.createElement("b");
      name.textContent = item.name;
      const chip = document.createElement("span");
      chip.className = "chip";
      b.append(glyph, name, chip);
      b.addEventListener("click", () => {
        host.querySelectorAll(".planet").forEach((p) => p.classList.toggle("selected", p === b));
        $("detail").hidden = false;
        $("detail").dataset.id = item.id;
        setText("d-title", item.name);
        setText("d-text", item.text);
        setText("d-stage", universeDetail(item));
      });
      return b;
    }),
  );
  updateUniverse();
}

// Chips follow the network: a built entry is live only while the node answers.
function updateUniverse() {
  const chainId = state.snapshot && state.snapshot.chain ? state.snapshot.chain.chain_id : null;
  $("universe").querySelectorAll(".planet").forEach((b) => {
    const item = UNIVERSE.find((u) => u.id === b.dataset.id);
    const chip = NetStatus.chip(item, state.net, chainId);
    b.className = `planet ${chip.tone}${b.classList.contains("selected") ? " selected" : ""}`;
    const el = b.querySelector(".chip");
    el.className = `chip ${chip.tone}`;
    setText(el, chip.text);
  });
  const open = UNIVERSE.find((u) => u.id === $("detail").dataset.id);
  if (open) setText("d-stage", universeDetail(open));
  renderLegend();
}

// ----------------------------------------------------------- animate ----

function frame(t) {
  const report = state.miner;
  const sum = report ? summarise(report.workers) : null;
  const last = sum && sum.running ? sum.hps : 0;
  const onMining = $("page-mining").classList.contains("active");
  const onNetwork = $("page-network").classList.contains("active");
  if (onMining) {
    Charts.gauge($("gauge"), { hps: last, peak: state.peak, active: !!(sum && sum.running) && !reducedMotion, t });
    Charts.drawBursts($("fx"));
  }
  // Reduced motion draws the network view only when its state changes (renderNode).
  if (onNetwork && !reducedMotion) NetView.frame(t);
  requestAnimationFrame(frame);
}

function showNetDetail(item) {
  $("n-detail").hidden = false;
  setText("nd-title", item.title);
  setText("nd-text", [item.line, item.detail].filter(Boolean).join(" "));
}

// Canvases read their colours from the tokens on every draw; a theme change
// redraws the ones that do not animate.
function redrawAll() {
  NetView.refresh();
  if (state.miner) renderMiner();
  renderNode();
  drawHeights();
  renderRigs();
}

// ------------------------------------------------------------ actions ----

async function poll(cmd, args, apply, every) {
  try {
    apply(await call(cmd, args()));
  } catch (err) {
    apply(null, err);
  }
  setTimeout(() => poll(cmd, args, apply, every), every);
}

const WATCH_ONLY = "Watching · mining runs on the desktop Hub";

// One device's start, from the saved settings. A refusal shows under Start.
async function startDevice(device) {
  const s = state.settings;
  const config = {
    binary: s.miner_binary,
    pool: s.pool || "public",
    key_file: s.key_file,
    threads: Number(s.cpu_threads),
    device: device === "cpu" ? null : Number(device.slice(3)),
    thermal_ceiling_c: Number(s.thermal_ceiling_c ?? DEFAULT_CEILING_C),
  };
  try {
    await call("miner_start", { config });
    state.peak = 0;
    state.startError = null;
  } catch (err) {
    state.startError = String(err);
  }
}

// The first device writes the payout key; the rest read it. Wait for its
// start line (the address) before starting the others, or they race it.
async function waitForPayoutKey(device) {
  for (let i = 0; i < 40; i++) {
    await new Promise((resolve) => setTimeout(resolve, 500));
    const report = await call("miner_state");
    state.miner = report;
    renderMiner();
    const w = report.workers.find((x) => x.device === device);
    if (!w || !w.running || w.address) return;
  }
}

async function startAll() {
  if (startReason()) return renderMiner();
  const [first, ...rest] = chosenDevices();
  await startDevice(first);
  await waitForPayoutKey(first);
  for (const device of rest) await startDevice(device);
}

async function togglePower() {
  if (state.miner && state.miner.workers.some((w) => w.running)) {
    await call("miner_stop");
    return;
  }
  // No miner runs before the first-start sheet has been finished once.
  if (!FirstRun.done()) return FirstRun.open();
  await startAll();
}

async function toggleRig(device) {
  const w = state.miner && state.miner.workers.find((x) => x.device === device);
  if (w && w.running) {
    try {
      await call("miner_stop", { device });
    } catch (err) {
      state.startError = String(err);
    }
    return;
  }
  if (!FirstRun.done()) return FirstRun.open();
  await startDevice(device);
}

function wireNav() {
  document.querySelectorAll(".nav").forEach((btn) =>
    btn.addEventListener("click", () => {
      document.querySelectorAll(".nav").forEach((b) => {
        b.classList.toggle("active", b === btn);
        if (b === btn) b.setAttribute("aria-current", "page");
        else b.removeAttribute("aria-current");
      });
      document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${btn.dataset.page}`));
      document.querySelector("main").scrollTop = 0;
      // Canvases on a hidden page measured zero; redraw once now visible.
      redrawAll();
    }),
  );
}

function renderDevicePicks(gpus) {
  const keys = ["cpu", ...gpus.map((g) => gpuKey(g.index))];
  const chosen = chosenDevices();
  $("s-devices").replaceChildren(
    ...keys.map((key) => {
      const label = document.createElement("label");
      label.className = "check";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.value = key;
      box.checked = chosen.includes(key);
      label.append(box, ` ${deviceLabel(key, gpuOf(key))}`);
      return label;
    }),
  );
}

async function saveSettingsForm() {
  const rpc = $("s-rpc").value.trim();
  if (!/^https?:\/\//i.test(rpc)) {
    setText("s-msg", "The node URL must start with http:// or https://");
    return;
  }
  const ceiling = Number($("s-ceiling").value);
  if (!(ceiling >= 50 && ceiling <= 95)) {
    setText("s-msg", "The temperature stop must be between 50 and 95 °C");
    return;
  }
  const devices = [...$("s-devices").querySelectorAll("input:checked")].map((box) => box.value);
  state.settings = {
    ...state.settings,
    rpc,
    pool: $("s-pool").value,
    miner_binary: $("s-binary").value.trim(),
    key_file: $("s-key").value.trim(),
    cpu_threads: Number($("s-threads").value),
    devices,
    thermal_ceiling_c: ceiling,
    notify: $("s-notify").checked,
  };
  const saved = saveSettings(state.settings) ? "Saved." : "Saved for this session only.";
  try {
    await call("notify_set", { on: state.settings.notify });
    setText("s-msg", saved);
  } catch (e) {
    setText("s-msg", `${saved} The system refused notifications: ${e}`);
  }
  renderNode();
}

function wireTheme() {
  const boxes = [...document.querySelectorAll("#s-theme input")];
  boxes.forEach((box) => {
    box.checked = box.value === Theme.get();
    box.addEventListener("change", () => box.checked && Theme.set(box.value));
  });
  Theme.onChange(redrawAll);
}

function wireSettings(defaults, gpus) {
  const s = state.settings;
  $("s-rpc").value = s.rpc;
  const pool = $("s-pool");
  pool.replaceChildren(
    ...defaults.pools.map((p) => {
      const option = document.createElement("option");
      option.value = p.id;
      option.textContent = p.name;
      return option;
    }),
  );
  pool.value = defaults.pools.some((p) => p.id === s.pool) ? s.pool : defaults.pools[0].id;
  // ADR-073 decision 5: other nodes are not offered until the miner checks the work itself.
  setText("s-pool-note", "Other nodes are not offered yet. A rig must check each share's work itself before it accepts another node's work (ADR-073, Phase 2).");
  $("s-binary").value = s.miner_binary;
  $("s-key").value = s.key_file;
  $("s-threads").max = String(Math.max(1, defaults.cpu_threads));
  $("s-threads").value = String(s.cpu_threads);
  setText("s-threads-v", s.cpu_threads);
  $("s-threads").addEventListener("input", (e) => setText("s-threads-v", e.target.value));
  $("s-ceiling").value = String(s.thermal_ceiling_c ?? DEFAULT_CEILING_C);
  $("s-notify").checked = Boolean(s.notify);
  // The Rust side starts with notifications off; send the saved choice once.
  if (!isPreview) call("notify_set", { on: Boolean(s.notify) }).catch((e) => setText("s-msg", `Notifications: ${e}`));
  renderDevicePicks(gpus.devices);
  $("s-save").addEventListener("click", saveSettingsForm);
  $("s-guide").addEventListener("click", () => FirstRun.open());
}

function finishFirstRun(devices) {
  state.settings = { ...state.settings, devices };
  saveSettings(state.settings);
  renderDevicePicks(state.gpus.devices);
  startAll();
}

async function main() {
  $("preview-flag").hidden = !isPreview;
  const defaults = await call("defaults");
  state.defaults = defaults;
  state.gpus = await call("gpu_telemetry").catch(() => ({ devices: [], note: "GPU telemetry unavailable" }));
  state.settings = loadSettings(defaults);
  wireNav();
  wireTheme();
  wireSettings(defaults, state.gpus);
  renderUniverse();
  Wallet.init({ call, url: () => state.settings.rpc });
  NetView.init({ canvas: $("orb"), layer: $("net-layer"), tip: $("net-tip"), reduced: reducedMotion, onSelect: showNetDetail });
  FirstRun.init({ call, isPreview, settings: () => state.settings, gpus: () => state.gpus.devices,
    label: (key) => deviceLabel(key, gpuOf(key)), net: () => state.net, onFinish: finishFirstRun });
  if (defaults.can_mine === false) {
    // Phones watch; the desktop Hub mines.
    $("power").hidden = true;
    $("s-miner").hidden = true;
    state.watchOnly = true;
  } else {
    $("power").addEventListener("click", togglePower);
    if (!FirstRun.done()) FirstRun.open();
  }

  const address = () => {
    const w = state.miner && state.miner.workers.find((x) => x.address && x.address !== "preview");
    return w ? w.address : null;
  };
  poll("miner_state", () => undefined, (m) => { if (m) { state.miner = m; renderMiner(); } }, POLL_MINER_MS);
  poll("node_snapshot", () => ({ url: state.settings.rpc, address: address() }), (s, err) => {
    state.snapshot = s || { reachable: false, error: String(err) };
    renderNode();
  }, POLL_NODE_MS);
  poll("gpu_telemetry", () => undefined, (g) => { if (g) { state.gpus = g; renderRigs(); } }, POLL_GPU_MS);
  requestAnimationFrame(frame);
}

main();
