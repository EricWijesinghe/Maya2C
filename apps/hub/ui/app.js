// Maya2C Hub: polling, state and rendering. All live numbers come from the
// Rust commands in src-tauri; opened in a plain browser, the page runs on
// synthetic preview data and says so in the header.
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

const tauri = window.__TAURI__ && window.__TAURI__.core;
const isPreview = !tauri;
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const $ = (id) => document.getElementById(id);

const state = {
  settings: null,
  miner: null, // { workers: [MinerState], alerts: [Alert] }
  snapshot: null,
  gpus: { devices: [], note: null },
  tune: { devices: [], notice: null }, // ADR-137: per-GPU auto-tuner state
  peak: 0,
  lastAccepted: 0,
  heights: [],
  lastHeight: null,
  rig: null, // device key of the rig the learn-more panel shows
  watchOnly: false,
};

// ---------------------------------------------------------------- data ----

const preview = (() => {
  const t0 = Date.now();
  const rates = [];
  let accepted = 0;
  let height = 241_000;
  const events = [];
  return {
    defaults: () => ({ rpc: "https://rpc.maya2c.dev/rpc", miner_binary: "maya2c-gpu-miner.exe", key_file: "miner.key", cpu_threads: 16, use_gpu: true, can_mine: true,
      pools: [{ id: "public", name: "Maya2C public node", url: "https://rpc.maya2c.dev/rpc" }], thermal_ceiling_c: DEFAULT_CEILING_C }),
    miner_state: () => {
      const t = Date.now();
      rates.push({ t_ms: t, hps: 6200 + 900 * Math.sin(t / 9000) + Math.random() * 400 });
      if (rates.length > 450) rates.shift();
      if (Math.random() < 0.05) {
        accepted += 1;
        events.push({ event: "share", accepted: true, reward: 7e22, t_ms: t });
      }
      return {
        workers: [{ device: "gpu0", running: true, status: "mining", mode: "preview", epoch: 118, share_reward: 7e22, accepted, rejected: 0,
          restarts: 0, held: false, temp_c: 60, ceiling_c: DEFAULT_CEILING_C, earned: accepted * 7e22, started_ms: t0, address: "preview", rates, events }],
        alerts: [],
      };
    },
    node_snapshot: () => {
      height += Math.random() < 0.6 ? 1 : 0;
      return { reachable: true, chain: { chain_id: "maya2c-preview", height }, fees: { active: true, base_fee: 1 },
        bft: { committee: 4, epoch: 118 }, mining: { active: true, epoch: 118, shares: 412, target_shares: 1000, share_reward: 7_000_000_000 },
        account: { balance: accepted * 7_000_000_000 } };
    },
    // Synthetic, like the rest of preview mode: the header already says so.
    watch_balances: ({ addresses }) => addresses.map((input, i) => ({ input, address_text: input,
      balance: (BigInt(i + 1) * 1234567n * 10n ** 21n).toString(), nonce: i, error: null })),
    gpu_telemetry: () => ({ devices: [{ index: 0, uuid: "GPU-preview", name: "Preview GPU", temp_c: 58 + Math.random() * 4, power_w: 170 + Math.random() * 15,
      power_limit_w: 250, util_pct: 97, fan_pct: 54, mem_used_mib: 4300, mem_total_mib: 12227, clock_mhz: 2610 }], note: null }),
    // Preview does not tune: the card says so instead of pretending.
    tune_report: () => ({ devices: [], notice: "preview mode: guided tuning runs only in the desktop Hub" }),
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
  const el = typeof id === "string" ? $(id) : id;
  if (el && el.textContent !== String(value)) el.textContent = value;
}

const deviceLabel = (key, gpu) => (key === "cpu" ? "CPU threads" : `GPU ${key.slice(3)}${gpu ? ` · ${gpu.name}` : ""}`);
const gpuKey = (index) => `gpu${index}`;

// ------------------------------------------------------------- summary ----

const lastRate = (w) => (w.rates && w.rates.length ? w.rates[w.rates.length - 1].hps : 0);
const sumOf = (list, key) => list.reduce((total, w) => total + (w[key] || 0), 0);

// The hero adds the rigs up. Every rig pays the same account, so the earned
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
    earned: workers.reduce((most, w) => Math.max(most, Number(w.earned) || 0), 0),
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
  if (ran < PACE_MIN_MS) return sum.running ? "paid rate after 5 min" : "";
  const perHour = (Number(sum.earned) * 3_600_000) / ran;
  return `≈ ${maya(perHour)} MAYA/h paid`;
}

// ------------------------------------------------------------- render ----

function renderMiner() {
  const report = state.miner;
  if (!report) return;
  const sum = summarise(report.workers);
  if (sum.hps) state.peak = Math.max(state.peak * 0.999, sum.hps * 1.1);
  const r = Charts.rate(sum.running ? sum.hps : null);
  setText("rate-value", r.value);
  setText("rate-unit", r.unit);
  setText("miner-status", state.watchOnly ? WATCH_ONLY : sum.status);
  setText("t-accepted", sum.accepted.toLocaleString());
  setText("t-rejected", sum.rejected.toLocaleString());
  setText("t-earned", maya(sum.earned));
  setText("t-epoch", sum.epoch ?? "—");
  setText("t-reward", sum.reward == null ? "—" : maya(sum.reward));
  setText("t-uptime", sum.running ? uptime(sum.started_ms) : "—");
  setText("t-pace", paceLine(sum));
  $("power").setAttribute("aria-pressed", String(sum.running));
  setText("power-text", sum.running ? "Stop mining" : "Start mining");
  // The node's snapshot carries the Bech32m form (ADR-081) and any refusal of it.
  const snap = state.snapshot || {};
  const payee = snap.address_error ? `Payout address refused: ${snap.address_error}`
    : sum.address ? `Paying to ${snap.address_text || sum.address}` : "";
  setText("payout-line", payee);

  if (sum.accepted > state.lastAccepted) {
    const fx = $("fx").getBoundingClientRect();
    if (!reducedMotion) Charts.burst(fx.width / 2, fx.height * 0.5, true);
    $("t-earned").parentElement.classList.remove("flash");
    void $("t-earned").parentElement.offsetWidth;
    $("t-earned").parentElement.classList.add("flash");
  }
  state.lastAccepted = sum.accepted;
  Charts.area($("rate-chart"), combinedRates(report.workers));
  renderFeed(report.workers);
  renderAlerts(report.alerts);
  renderRigs();
}

function renderFeed(workers) {
  const shares = workers
    .flatMap((w) => w.events.map((e) => ({ ...e, device: w.device })))
    .filter((e) => e.event === "share" || e.event === "error")
    .sort((a, b) => a.t_ms - b.t_ms)
    .slice(-FEED_ROWS)
    .reverse();
  const feed = $("feed");
  if (!shares.length) return;
  const key = shares.map((e) => `${e.device}${e.t_ms}`).join(",");
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
      const text = ok ? `Share sent · pays up to +${maya(e.reward)} MAYA when it lands` : e.reason || e.message || "refused";
      what.textContent = `${e.device} · ${text}`;
      time.textContent = clock(e.t_ms);
      li.append(mark, what, time);
      return li;
    }),
  );
}

const LEVEL_WORD = { info: "Info", warn: "Warning", critical: "Stopped" };
const LEVEL_ADVICE = {
  info: "Normal recovery. Nothing to do.",
  warn: "The Hub handles this on its own. The alert stays so you can see that it happened.",
  critical: "The Hub will not restart this device by itself. Check the pool and the miner program, then press Start on the rig.",
};

function renderAlerts(alerts) {
  setText("alert-count", alerts.length ? String(alerts.length) : "");
  const host = $("alerts");
  const key = alerts.map((a) => `${a.t_ms}${a.message}`).join("|");
  if (host.dataset.key === key) return;
  host.dataset.key = key;
  if (!alerts.length) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No alerts. Crashes, restarts and heat stops appear here.";
    host.replaceChildren(li);
    return;
  }
  host.replaceChildren(
    ...alerts.slice(0, ALERT_ROWS).map((a) => {
      const li = document.createElement("li");
      const chip = document.createElement("span");
      const what = document.createElement("span");
      const time = document.createElement("time");
      chip.className = `lvl ${a.level}`;
      chip.textContent = LEVEL_WORD[a.level] || a.level;
      what.textContent = a.message;
      time.textContent = clock(a.t_ms);
      li.append(chip, what, time);
      li.tabIndex = 0;
      li.addEventListener("click", () => showAlert(a));
      return li;
    }),
  );
}

function showAlert(a) {
  $("rig-detail").hidden = false;
  setText("rd-title", `${deviceLabel(a.device, gpuOf(a.device))} · ${LEVEL_WORD[a.level] || a.level}`);
  setText("rd-text", `${a.message}. ${LEVEL_ADVICE[a.level] || ""}`);
}

// A rig's phase, as one chip: the watchdog's view in words.
function phaseOf(w) {
  if (!w) return ["idle", "Not started"];
  if (w.held) return ["warn", "Held: too hot"];
  if (w.running) return ["live", w.status || "Mining"];
  if (w.status && w.status.startsWith("stopped:")) return ["crit", "Stopped: crash loop"];
  if (w.status && w.status.includes("restarting")) return ["warn", w.status];
  return ["idle", w.status || "Stopped"];
}

const gpuOf = (key) => state.gpus.devices.find((g) => gpuKey(g.index) === key) || null;

function rigKeys(workers) {
  const keys = new Set(["cpu", ...workers.map((w) => w.device), ...state.gpus.devices.map((g) => gpuKey(g.index))]);
  return [...keys].sort((a, b) => (a === "cpu" ? -1 : b === "cpu" ? 1 : Number(a.slice(3)) - Number(b.slice(3))));
}

// One card per device: the CPU and every GPU the driver reports, mining or not.
function renderRigs() {
  const workers = state.miner ? state.miner.workers : [];
  const byKey = new Map(workers.map((w) => [w.device, w]));
  setText("gpu-note", state.gpus.note || "");
  const host = $("devices");
  for (const key of rigKeys(workers)) {
    let card = host.querySelector(`[data-device="${key}"]`);
    if (!card) {
      card = rigCard(key);
      host.append(card);
    }
    updateRig(card, key, byKey.get(key) || null, gpuOf(key));
  }
  if (state.rig) showRig(state.rig);
}

function rigCard(key) {
  const card = document.createElement("div");
  card.className = "device";
  card.dataset.device = key;
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  const h3 = document.createElement("h3");
  const name = document.createElement("span");
  name.className = "rig-name";
  const phase = document.createElement("span");
  phase.className = "chip idle";
  h3.append(name, phase);
  const spark = document.createElement("canvas");
  spark.className = "spark";
  const rings = document.createElement("div");
  rings.className = "rings";
  ["Temp", "Power", "Load", "Fan"].forEach((label) => {
    const cell = document.createElement("div");
    const text = document.createElement("label");
    text.textContent = label;
    cell.append(document.createElement("canvas"), text);
    rings.append(cell);
  });
  const meta = document.createElement("div");
  meta.className = "meta";
  const counts = document.createElement("span");
  counts.className = "rig-counts";
  const heat = document.createElement("span");
  heat.className = "rig-heat";
  const btn = document.createElement("button");
  btn.className = "rig-btn";
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    toggleRig(key);
  });
  const tune = document.createElement("button");
  tune.className = "tune-btn";
  tune.type = "button";
  tune.setAttribute("aria-pressed", "false");
  tune.textContent = "Tune";
  tune.addEventListener("click", (e) => {
    e.stopPropagation();
    toggleTune(key);
  });
  const tuneLine = document.createElement("p");
  tuneLine.className = "tune-line";
  // The command for the limit to set now. Shown as text to copy; the Hub never runs it.
  const tuneCmd = document.createElement("code");
  tuneCmd.className = "tune-cmd";
  const eff = document.createElement("span");
  eff.className = "rig-eff";
  meta.append(counts, heat, eff);
  card.append(h3, spark, rings, meta, tune, tuneLine, tuneCmd, btn);
  card.addEventListener("click", () => showRig(key));
  card.addEventListener("keydown", (e) => {
    if (e.key === "Enter") showRig(key);
  });
  return card;
}

function updateRig(card, key, w, g) {
  setText(card.querySelector(".rig-name"), deviceLabel(key, g));
  const [tone, phrase] = phaseOf(w);
  const chip = card.querySelector(".chip");
  chip.className = `chip ${tone}`;
  setText(chip, phrase);
  const rings = card.querySelector(".rings");
  rings.hidden = !g;
  if (g) {
    const cans = rings.querySelectorAll("canvas");
    Charts.ring(cans[0], { value: g.temp_c, max: 100, warn: 83, text: `${Math.round(g.temp_c)}°` });
    Charts.ring(cans[1], { value: g.power_w, max: g.power_limit_w || 300, text: `${Math.round(g.power_w)}W` });
    Charts.ring(cans[2], { value: g.util_pct, max: 100, text: `${Math.round(g.util_pct)}%` });
    Charts.ring(cans[3], { value: g.fan_pct, max: 100, text: `${Math.round(g.fan_pct)}%` });
  }
  Charts.area(card.querySelector("canvas.spark"), w ? w.rates : []);
  setText(card.querySelector(".rig-counts"), w ? `${w.accepted} accepted · ${w.rejected} rejected · ${w.restarts} restarts` : "No shares yet");
  const hot = g && g.temp_c != null ? `${Math.round(g.temp_c)} °C · stop above ${w && w.ceiling_c ? w.ceiling_c : DEFAULT_CEILING_C} °C` : "";
  setText(card.querySelector(".rig-heat"), hot);
  const btn = card.querySelector(".rig-btn");
  btn.hidden = state.watchOnly;
  setText(btn, w && w.running ? "Stop" : "Start");
  // Efficiency read-out (ADR-137 T0): hash rate per watt drawn, only from real readings.
  const hps = w && w.running ? lastRate(w) : 0;
  const watts = g && Number.isFinite(g.power_w) ? g.power_w : 0;
  setText(card.querySelector(".rig-eff"), hps > 0 && watts > 0 ? ` · ${(hps / watts).toFixed(2)} H/s per W` : "");
  updateTune(card, g);
}

// The Tune toggle and its guided steps. The Hub suggests a power limit, you
// set it, the Hub reads it back and measures. It never changes the card.
function updateTune(card, g) {
  const t = (g && g.uuid && state.tune.devices.find((d) => d.uuid === g.uuid)) || null;
  const tune = card.querySelector(".tune-btn");
  const on = Boolean(t && t.on);
  tune.setAttribute("aria-pressed", String(on));
  tune.classList.toggle("on", on);
  // The tuner keys every result by the driver's UUID: no UUID, no tuning.
  tune.disabled = state.watchOnly || !g || !g.uuid;
  const saved = t && t.saved_w ? ` Last result for this card: ${t.saved_w} W.` : "";
  setText(card.querySelector(".tune-line"), t ? t.line + saved : "Tune is off. Press Tune for a guided power-limit search.");
  // A command the user may paste as administrator: only from a well-formed UUID and a real number.
  const safe = t && Number.isFinite(t.set_w) && /^GPU-[0-9a-fA-F-]+$/.test(g.uuid);
  const cmd = safe ? `nvidia-smi -i ${g.uuid} -pl ${Math.round(t.set_w)}` : "";
  const code = card.querySelector(".tune-cmd");
  code.hidden = !cmd;
  code.title = "Run in an administrator terminal, or set the same watts in your GPU tool";
  setText(code, cmd);
}

function showRig(key) {
  state.rig = key;
  const w = (state.miner ? state.miner.workers : []).find((x) => x.device === key) || null;
  const g = gpuOf(key);
  const parts = [`${phaseOf(w)[1]}.`];
  if (g && g.temp_c != null) parts.push(`Running at ${Math.round(g.temp_c)} °C. The Hub stops this GPU above ${w && w.ceiling_c ? w.ceiling_c : DEFAULT_CEILING_C} °C and restarts it 5 °C lower.`);
  if (w) parts.push(`Since start: ${w.accepted} shares accepted, ${w.rejected} rejected, ${w.restarts} automatic restart${w.restarts === 1 ? "" : "s"}.`);
  parts.push(key === "cpu" ? "Hashing on CPU threads." : "Hashing on this graphics card.");
  $("rig-detail").hidden = false;
  setText("rd-title", deviceLabel(key, g));
  setText("rd-text", parts.join(" "));
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

// One device's start, from the saved settings. Errors show in the status line.
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
  } catch (err) {
    setText("miner-status", String(err));
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

async function togglePower() {
  if (state.miner && state.miner.workers.some((w) => w.running)) {
    await call("miner_stop");
    return;
  }
  const [first, ...rest] = (state.settings.devices || []).filter(Boolean);
  if (!first) {
    setText("miner-status", "Pick at least one device under Settings");
    return;
  }
  await startDevice(first);
  await waitForPayoutKey(first);
  for (const device of rest) await startDevice(device);
}

// Tune is opt-in per card and visible on it. Off is the default; the backend
// takes the card's UUID and the floor, never a watt value (ADR-137 R-1).
async function toggleTune(key) {
  const g = gpuOf(key);
  if (!g || !g.uuid) return;
  const on = !(state.tune.devices.find((d) => d.uuid === g.uuid) || { on: false }).on;
  try {
    await call("tune_set", { uuid: g.uuid, on, floorPct: tuneFloor(), ceilingC: state.settings.thermal_ceiling_c });
  } catch (err) {
    setText("gpu-note", String(err));
  }
  await refreshTune();
}

// The hash-rate floor slider (ADR-137 O-3): keep at least this share of the
// default-limit hash rate. Default 95 %; the backend clamps it to 80-100 %.
const TUNE_FLOOR_DEFAULT = 95;
const tuneFloor = () => Number(state.settings.tune_floor_pct ?? TUNE_FLOOR_DEFAULT);

function wireTuneFloor() {
  const slider = $("tune-floor");
  const show = () => setText("tune-floor-out", `${slider.value}%`);
  slider.value = String(tuneFloor());
  show();
  slider.addEventListener("input", show);
  slider.addEventListener("change", () => {
    state.settings = { ...state.settings, tune_floor_pct: Number(slider.value) };
    saveSettings(state.settings);
  });
}

async function refreshTune() {
  const r = await call("tune_report").catch(() => null);
  if (r) {
    state.tune = r;
    renderRigs();
  }
}

async function toggleRig(device) {
  const w = state.miner && state.miner.workers.find((x) => x.device === device);
  if (w && w.running) {
    try {
      await call("miner_stop", { device });
    } catch (err) {
      setText("miner-status", String(err));
    }
    return;
  }
  await startDevice(device);
}

function wireNav() {
  document.querySelectorAll(".nav").forEach((btn) =>
    btn.addEventListener("click", () => {
      document.querySelectorAll(".nav").forEach((b) => b.classList.toggle("active", b === btn));
      document.querySelectorAll(".page").forEach((p) => p.classList.toggle("active", p.id === `page-${btn.dataset.page}`));
      // Canvases on a hidden page measured zero; redraw once now visible.
      if (state.miner) renderMiner();
      renderNode();
      renderRigs();
    }),
  );
}

function renderDevicePicks(gpus) {
  const keys = ["cpu", ...gpus.map((g) => gpuKey(g.index))];
  const chosen = state.settings.devices || keys.filter((k) => k !== "cpu" || !gpus.length);
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
}

async function main() {
  $("preview-flag").hidden = !isPreview;
  const defaults = await call("defaults");
  state.gpus = await call("gpu_telemetry").catch(() => ({ devices: [], note: "GPU telemetry unavailable" }));
  state.settings = loadSettings(defaults);
  wireNav();
  wireSettings(defaults, state.gpus);
  wireTuneFloor();
  renderUniverse();
  Wallet.init({ call, url: () => state.settings.rpc });
  NetView.init({ canvas: $("orb"), layer: $("net-layer"), tip: $("net-tip"), reduced: reducedMotion, onSelect: showNetDetail });
  if (defaults.can_mine === false) {
    // Phones watch; the desktop Hub mines.
    $("power").hidden = true;
    $("s-miner").hidden = true;
    state.watchOnly = true;
  } else {
    $("power").addEventListener("click", togglePower);
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
  poll("tune_report", () => undefined, (r) => {
    if (r) {
      state.tune = r;
      setText("tune-note", r.notice || "");
      renderRigs();
    }
  }, POLL_GPU_MS);
  requestAnimationFrame(frame);
}

main();
