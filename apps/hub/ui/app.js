// Maya2C Hub: polling, state and rendering. All live numbers come from the
// Rust commands in src-tauri; opened in a plain browser, the page runs on
// synthetic preview data and says so in the header.
"use strict";

const BASE_PER_MAYA = 1e24; // 24 decimals (ADR-068): 21M MAYA = 2.1e31 base units
const POLL_MINER_MS = 1000;
const POLL_NODE_MS = 4000;
const POLL_GPU_MS = 3000;
const HEIGHT_HISTORY = 300;

const tauri = window.__TAURI__ && window.__TAURI__.core;
const isPreview = !tauri;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const $ = (id) => document.getElementById(id);

const state = {
  settings: null,
  miner: null,
  snapshot: null,
  gpus: { devices: [], note: null },
  peak: 0,
  lastAccepted: 0,
  heights: [],
  lastHeight: null,
};

// ---------------------------------------------------------------- data ----

const preview = (() => {
  const t0 = Date.now();
  const rates = [];
  let accepted = 0;
  let height = 241_000;
  const events = [];
  return {
    defaults: () => ({ rpc: "https://rpc.maya2c.dev/rpc", miner_binary: "maya2c-gpu-miner.exe", key_file: "miner.key", cpu_threads: 16, use_gpu: true, can_mine: true }),
    miner_state: () => {
      const t = Date.now();
      rates.push({ t_ms: t, hps: 6200 + 900 * Math.sin(t / 9000) + Math.random() * 400 });
      if (rates.length > 450) rates.shift();
      if (Math.random() < 0.05) {
        accepted += 1;
        events.push({ event: "share", accepted: true, reward: 7_000_000_000, t_ms: t });
      }
      return { running: true, status: "mining", mode: "preview", epoch: 118, share_reward: 7_000_000_000, accepted, rejected: 0,
        earned: accepted * 7_000_000_000, started_ms: t0, address: "preview", rates, events };
    },
    node_snapshot: () => {
      height += Math.random() < 0.6 ? 1 : 0;
      return { reachable: true, chain: { chain_id: "maya2c-preview", height }, fees: { active: true, base_fee: 1 },
        bft: { committee: 4, epoch: 118 }, mining: { active: true, epoch: 118, shares: 412, target_shares: 1000, share_reward: 7_000_000_000 },
        account: { balance: accepted * 7_000_000_000 } };
    },
    gpu_telemetry: () => ({ devices: [{ index: 0, name: "Preview GPU", temp_c: 58 + Math.random() * 4, power_w: 170 + Math.random() * 15,
      power_limit_w: 250, util_pct: 97, fan_pct: 54, mem_used_mib: 4300, mem_total_mib: 12227, clock_mhz: 2610 }] }),
  };
})();

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
  const el = $(id);
  if (el && el.textContent !== String(value)) el.textContent = value;
}

// ------------------------------------------------------------- render ----

function renderMiner() {
  const m = state.miner;
  if (!m) return;
  const last = m.rates.length ? m.rates[m.rates.length - 1].hps : null;
  if (last) state.peak = Math.max(state.peak * 0.999, last * 1.1);
  const r = Charts.rate(m.running ? last : null);
  setText("rate-value", r.value);
  setText("rate-unit", r.unit);
  setText("miner-status", state.watchOnly ? WATCH_ONLY : m.status || (m.running ? "running" : "stopped"));
  setText("t-accepted", m.accepted.toLocaleString());
  setText("t-rejected", m.rejected.toLocaleString());
  setText("t-earned", maya(m.earned));
  setText("t-epoch", m.epoch ?? "—");
  setText("t-reward", m.share_reward == null ? "—" : maya(m.share_reward));
  setText("t-uptime", m.running ? uptime(m.started_ms) : "—");
  const power = $("power");
  power.setAttribute("aria-pressed", String(m.running));
  setText("power-text", m.running ? "Stop mining" : "Start mining");
  setText("payout-line", m.address && m.address !== "preview" ? `Paying to ${m.address}` : "");

  if (m.accepted > state.lastAccepted) {
    const fx = $("fx").getBoundingClientRect();
    if (!reducedMotion) Charts.burst(fx.width / 2, fx.height * 0.5, true);
    $("t-earned").parentElement.classList.remove("flash");
    void $("t-earned").parentElement.offsetWidth;
    $("t-earned").parentElement.classList.add("flash");
  }
  state.lastAccepted = m.accepted;
  Charts.area($("rate-chart"), m.rates);
  renderFeed(m.events);
}

function renderFeed(events) {
  const shares = events.filter((e) => e.event === "share" || e.event === "error").slice(-30).reverse();
  const feed = $("feed");
  if (!shares.length) return;
  const key = shares.map((e) => e.t_ms).join(",");
  if (feed.dataset.key === key) return;
  feed.dataset.key = key;
  feed.replaceChildren(
    ...shares.map((e) => {
      const li = document.createElement("li");
      const mark = document.createElement("span");
      const what = document.createElement("span");
      const time = document.createElement("time");
      const ok = e.event === "share" && e.accepted;
      mark.className = ok ? "ok" : "bad";
      mark.textContent = ok ? "✓" : "✕";
      what.textContent = ok ? `Share sent · pays up to +${maya(e.reward)} MAYA when it lands` : e.reason || e.message || "refused";
      time.textContent = clock(e.t_ms);
      li.append(mark, what, time);
      return li;
    }),
  );
}

function renderNode() {
  const s = state.snapshot;
  const pill = $("net-pill");
  if (!s || !s.reachable) {
    pill.className = "pill down";
    setText("net-text", s && s.error ? "Node unreachable" : "Connecting…");
    NetView.update(s, null, false);
    NetView.frame(performance.now());
    return;
  }
  const height = s.chain.height;
  pill.className = "pill live";
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
  Charts.area($("height-chart"), state.heights, { floor: true, label: (v) => ({ value: Math.round(v).toLocaleString(), unit: "" }) });

  setText("t-balance", s.account ? maya(s.account.balance) : "—");
  if (work && work.active) {
    setText("t-epoch-shares", `${work.shares} / ${work.target_shares}`);
    $("t-epoch-bar").style.width = `${Math.min(100, (100 * work.shares) / Math.max(1, work.target_shares))}%`;
  }
}

function renderGpus() {
  const { devices, note } = state.gpus;
  setText("gpu-note", note || "");
  const host = $("devices");
  if (!devices.length) {
    if (!host.querySelector(".empty-devices")) {
      const p = document.createElement("p");
      p.className = "empty-devices";
      p.textContent = "No GPU telemetry. The miner still runs on CPU threads.";
      host.replaceChildren(p);
    }
    return;
  }
  devices.forEach((g) => {
    let card = host.querySelector(`[data-gpu="${g.index}"]`);
    if (!card) {
      if (host.querySelector(".empty-devices")) host.replaceChildren();
      card = deviceCard(g);
      host.append(card);
    }
    const rings = card.querySelectorAll("canvas");
    Charts.ring(rings[0], { value: g.temp_c, max: 100, warn: 83, text: `${Math.round(g.temp_c)}°` });
    Charts.ring(rings[1], { value: g.power_w, max: g.power_limit_w || 300, text: `${Math.round(g.power_w)}W` });
    Charts.ring(rings[2], { value: g.util_pct, max: 100, text: `${Math.round(g.util_pct)}%` });
    Charts.ring(rings[3], { value: g.fan_pct, max: 100, text: `${Math.round(g.fan_pct)}%` });
    const mem = g.mem_used_mib != null ? `${(g.mem_used_mib / 1024).toFixed(1)} / ${(g.mem_total_mib / 1024).toFixed(1)} GiB` : "—";
    setText(`gpu-mem-${g.index}`, `VRAM ${mem}`);
    setText(`gpu-clk-${g.index}`, g.clock_mhz != null ? `${Math.round(g.clock_mhz)} MHz` : "—");
  });
}

function deviceCard(g) {
  const card = document.createElement("div");
  card.className = "device";
  card.dataset.gpu = g.index;
  const h3 = document.createElement("h3");
  h3.textContent = g.name;
  const idx = document.createElement("span");
  idx.textContent = `GPU ${g.index}`;
  h3.append(idx);
  const rings = document.createElement("div");
  rings.className = "rings";
  ["Temp", "Power", "Load", "Fan"].forEach((name) => {
    const cell = document.createElement("div");
    const label = document.createElement("label");
    label.textContent = name;
    cell.append(document.createElement("canvas"), label);
    rings.append(cell);
  });
  const meta = document.createElement("div");
  meta.className = "meta";
  const mem = document.createElement("span");
  mem.id = `gpu-mem-${g.index}`;
  const clk = document.createElement("span");
  clk.id = `gpu-clk-${g.index}`;
  meta.append(mem, clk);
  card.append(h3, rings, meta);
  return card;
}

function renderUniverse() {
  const host = $("universe");
  host.replaceChildren(
    ...UNIVERSE.map((item) => {
      const b = document.createElement("button");
      b.className = `planet ${item.status}`;
      const glyph = document.createElement("span");
      glyph.className = "glyph";
      glyph.textContent = item.icon;
      const name = document.createElement("b");
      name.textContent = item.name;
      const chip = document.createElement("span");
      chip.className = `chip ${item.status}`;
      chip.textContent = { live: "Live on testnet", building: "Building", research: "Research", planned: "Planned" }[item.status];
      b.append(glyph, name, chip);
      b.addEventListener("click", () => {
        host.querySelectorAll(".planet").forEach((p) => p.classList.remove("selected"));
        b.classList.add("selected");
        $("detail").hidden = false;
        setText("d-title", item.name);
        setText("d-text", item.text);
      });
      return b;
    }),
  );
}

// ----------------------------------------------------------- animate ----

function frame(t) {
  const m = state.miner;
  const last = m && m.running && m.rates.length ? m.rates[m.rates.length - 1].hps : 0;
  const onMining = $("page-mining").classList.contains("active");
  const onNetwork = $("page-network").classList.contains("active");
  if (onMining) {
    Charts.gauge($("gauge"), { hps: last, peak: state.peak, active: !!(m && m.running) && !reducedMotion, t });
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

// ------------------------------------------------------------ actions ----

async function poll(cmd, args, apply, every) {
  try {
    apply(await call(cmd, args()));
  } catch (err) {
    apply(null, err);
  }
  setTimeout(() => poll(cmd, args, apply, every), every);
}

const WATCH_ONLY = "Watching — mining runs on the desktop Hub";

async function togglePower() {
  const running = state.miner && state.miner.running;
  try {
    if (running) await call("miner_stop");
    else {
      const s = state.settings;
      await call("miner_start", { config: { binary: s.miner_binary, rpc: s.rpc, key_file: s.key_file, threads: Number(s.cpu_threads), gpu: s.use_gpu !== false } });
      state.peak = 0;
    }
  } catch (err) {
    setText("miner-status", String(err));
  }
}

function wireNav() {
  document.querySelectorAll(".nav").forEach((btn) =>
    btn.addEventListener("click", () => {
      document.querySelectorAll(".nav").forEach((b) => b.classList.toggle("active", b === btn));
      document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${btn.dataset.page}`));
      // Canvases on a hidden page measured zero; redraw once now visible.
      if (state.miner) renderMiner();
      renderNode();
      renderGpus();
    }),
  );
}

function wireSettings(maxThreads) {
  const s = state.settings;
  $("s-rpc").value = s.rpc;
  $("s-binary").value = s.miner_binary;
  $("s-key").value = s.key_file;
  $("s-threads").max = String(Math.max(1, maxThreads));
  $("s-threads").value = String(s.cpu_threads);
  $("s-gpu").checked = s.use_gpu !== false;
  setText("s-threads-v", s.cpu_threads);
  $("s-threads").addEventListener("input", (e) => setText("s-threads-v", e.target.value));
  $("s-save").addEventListener("click", () => {
    const rpc = $("s-rpc").value.trim();
    if (!/^https?:\/\//i.test(rpc)) {
      setText("s-msg", "The node URL must start with http:// or https://");
      return;
    }
    state.settings = { rpc, miner_binary: $("s-binary").value.trim(), key_file: $("s-key").value.trim(), cpu_threads: Number($("s-threads").value), use_gpu: $("s-gpu").checked };
    setText("s-msg", saveSettings(state.settings) ? "Saved." : "Saved for this session only.");
  });
}

async function main() {
  $("preview-flag").hidden = !isPreview;
  const defaults = await call("defaults");
  state.settings = loadSettings(defaults);
  wireNav();
  wireSettings(defaults.cpu_threads);
  renderUniverse();
  NetView.init({ canvas: $("orb"), layer: $("net-layer"), tip: $("net-tip"), reduced: reducedMotion, onSelect: showNetDetail });
  if (defaults.can_mine === false) {
    // Phones watch; the desktop Hub mines.
    $("power").hidden = true;
    $("s-miner").hidden = true;
    state.watchOnly = true;
  } else {
    $("power").addEventListener("click", togglePower);
  }

  const address = () => (state.miner && state.miner.address && state.miner.address !== "preview" ? state.miner.address : null);
  poll("miner_state", () => undefined, (m) => { if (m) { state.miner = m; renderMiner(); } }, POLL_MINER_MS);
  poll("node_snapshot", () => ({ url: state.settings.rpc, address: address() }), (s, err) => {
    state.snapshot = s || { reachable: false, error: String(err) };
    renderNode();
  }, POLL_NODE_MS);
  poll("gpu_telemetry", () => undefined, (g) => { if (g) { state.gpus = g; renderGpus(); } }, POLL_GPU_MS);
  requestAnimationFrame(frame);
}

main();
