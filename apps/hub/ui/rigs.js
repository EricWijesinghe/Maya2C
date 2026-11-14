// Mining page lists: the share feed, the alerts and one card per device.
// Every figure comes from miner_state and gpu_telemetry, which app.js polls.
"use strict";

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
      const ok = e.event === "share" && e.accepted;
      const mark = icon(ok ? "check" : "cross", `icon mark ${ok ? "ok" : "bad"}`);
      const what = document.createElement("span");
      const time = document.createElement("time");
      // No amount: what an accepted share may pay is not what the chain paid.
      const text = ok ? "Share accepted" : e.reason || e.message || "refused";
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
      chip.className = `chip ${a.level === "critical" ? "danger" : a.level === "warn" ? "warn" : "info"}`;
      chip.textContent = LEVEL_WORD[a.level] || a.level;
      what.textContent = a.message;
      time.textContent = clock(a.t_ms);
      li.append(chip, what, time);
      li.tabIndex = 0;
      li.addEventListener("click", () => showAlert(a));
      li.addEventListener("keydown", (e) => {
        if (e.key === "Enter") showAlert(a);
      });
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
  if (w.status && w.status.startsWith("stopped:")) return ["danger", "Stopped: crash loop"];
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

const RINGS = [["Temp", "Temperature"], ["Power", "Power draw"], ["Load", "Load"], ["Fan", "Fan speed"]];

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
  h3.append(icon("chip"), name, phase);
  const spark = document.createElement("canvas");
  spark.className = "spark";
  // Named from the start; charts.js replaces the name with the value on draw.
  Charts.describe(spark, `${key} hash rate: no samples yet`);
  const rings = document.createElement("div");
  rings.className = "rings";
  RINGS.forEach(([label, spoken]) => {
    const cell = document.createElement("div");
    const text = document.createElement("span");
    text.className = "label";
    text.textContent = label;
    const can = document.createElement("canvas");
    Charts.describe(can, `${key} ${spoken.toLowerCase()}: not reported yet`);
    cell.append(can, text);
    rings.append(cell);
  });
  const meta = document.createElement("div");
  meta.className = "meta";
  const counts = document.createElement("span");
  counts.className = "rig-counts";
  const heat = document.createElement("span");
  heat.className = "rig-heat";
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "btn secondary rig-btn";
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    toggleRig(key);
  });
  meta.append(counts, heat);
  card.append(h3, spark, rings, meta, btn);
  card.addEventListener("click", () => showRig(key));
  card.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && e.target === card) showRig(key);
  });
  return card;
}

function updateRig(card, key, w, g) {
  const label = deviceLabel(key, g);
  setText(card.querySelector(".rig-name"), label);
  const [tone, phrase] = phaseOf(w);
  const chip = card.querySelector(".chip");
  chip.className = `chip ${tone}`;
  setText(chip, phrase);
  const rings = card.querySelector(".rings");
  rings.hidden = !g;
  if (g) {
    const cans = rings.querySelectorAll("canvas");
    const values = [
      { value: g.temp_c, max: 100, warn: 83, text: `${Math.round(g.temp_c)}°`, spoken: `${Math.round(g.temp_c)} °C` },
      { value: g.power_w, max: g.power_limit_w || 300, text: `${Math.round(g.power_w)}W`, spoken: `${Math.round(g.power_w)} W` },
      { value: g.util_pct, max: 100, text: `${Math.round(g.util_pct)}%`, spoken: `${Math.round(g.util_pct)} %` },
      { value: g.fan_pct, max: 100, text: `${Math.round(g.fan_pct)}%`, spoken: `${Math.round(g.fan_pct)} %` },
    ];
    values.forEach((v, i) => Charts.ring(cans[i], { ...v, name: `${label} ${RINGS[i][1].toLowerCase()}` }));
  }
  Charts.area(card.querySelector("canvas.spark"), w ? w.rates : [], { name: `${label} hash rate`, empty: "No samples yet" });
  setText(card.querySelector(".rig-counts"), w ? `${w.accepted} accepted · ${w.rejected} rejected · ${w.restarts} restarts` : "No shares yet");
  const hot = g && g.temp_c != null ? `${Math.round(g.temp_c)} °C · stop above ${w && w.ceiling_c ? w.ceiling_c : DEFAULT_CEILING_C} °C` : "";
  setText(card.querySelector(".rig-heat"), hot);
  const btn = card.querySelector(".rig-btn");
  btn.hidden = state.watchOnly;
  const running = Boolean(w && w.running);
  setText(btn, running ? "Stop" : "Start");
  const block = running ? null : startReason();
  btn.disabled = Boolean(block);
  btn.title = block || "";
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
