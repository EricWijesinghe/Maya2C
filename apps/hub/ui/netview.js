// Network health view: a 3D orbit drawn on canvas, no libraries. Every figure
// comes from the node snapshot. Validators are anonymous because the node
// reports only a count, so they are drawn without names. Each element also has
// a focusable button on top of the canvas, so keyboard users get the same text.
"use strict";

const NetView = (() => {
  const TILT = 0.42; // orbit plane tilt, radians
  const CAMERA = 3.2; // camera distance in orbit radii; sets the perspective
  const ORBIT_FRAC = 0.4; // orbit radius as a share of the shorter canvas side
  const FLASH_MS = 900; // core flash and outward ring when a new block arrives
  const LIT_MS = 800; // validators glow this long after a block's pulse reaches them
  const ORBIT_PER_MS = 0.0004; // radians per ms: one lap about every 16 s
  const SPIN_PER_MS = 0.00006; // slow turn of the whole scene
  const MAX_DRAWN = 32; // validators beyond this are counted but not drawn
  const PULSE_BASE_MS = 2600; // travel time when the block rate is unknown
  const PULSE_MIN_MS = 900;
  const PULSE_MAX_MS = 4000;
  const HIT_PX = 36; // touch and focus target, larger than the drawn dot
  const SEGMENTS = 72;
  const TIP_BELOW_PX = 110; // tooltips under this height open downward

  const v = {
    canvas: null,
    layer: null,
    tip: null,
    onSelect: null,
    reduced: false,
    colors: null,
    snap: null,
    rate: null,
    pulses: [],
    flash: -Infinity, // performance.now() of the last new block
    lit: -Infinity, // when the last pulse reached the committee
    entries: [],
    active: null,
  };

  // ------------------------------------------------------------ geometry ----

  // A point on a ring of unit radius, tilted about X then turned about Y.
  function place(ang, radius, tilt, spin) {
    const x0 = radius * Math.cos(ang);
    const y0 = radius * Math.sin(ang);
    const z1 = y0 * Math.sin(tilt);
    return {
      x: x0 * Math.cos(spin) + z1 * Math.sin(spin),
      y: y0 * Math.cos(tilt),
      z: -x0 * Math.sin(spin) + z1 * Math.cos(spin),
    };
  }

  // Perspective projection; z is in orbit radii, so farther points get k < 1.
  function project(p, g) {
    const k = CAMERA / (CAMERA + p.z);
    return { x: g.cx + p.x * g.R * k, y: g.cy + p.y * g.R * k, k, z: p.z };
  }

  function committeeCount(s) {
    return s && s.bft && s.bft.committee != null ? Number(s.bft.committee) : null;
  }

  // Block-to-validator travel time: a higher measured rate gives faster pulses.
  function travelMs(rate) {
    if (rate == null || rate <= 0) return PULSE_BASE_MS;
    return Math.min(PULSE_MAX_MS, Math.max(PULSE_MIN_MS, (PULSE_BASE_MS * 6) / rate));
  }

  // ---------------------------------------------------------------- text ----

  function describeCore(s, rate) {
    if (!s) return { title: "Connecting", line: "Waiting for the first reply from the node." };
    if (!s.reachable) return { title: "No reply from the node", line: "The node did not answer the last poll. The view stays dimmed until it does." };
    const fee = s.fees && s.fees.active ? `base fee ${s.fees.base_fee} per byte` : "fees off";
    const measured = rate == null ? "not measured yet" : `${rate.toFixed(1)} blocks/min, measured here`;
    return {
      title: `${s.chain.chain_id} · #${Number(s.chain.height).toLocaleString()}`,
      line: "Each new block sends a pulse out to every validator.",
      detail: `Pulse speed follows the block rate (${measured}); ${fee}. Source: node snapshot.`,
    };
  }

  function describeCommittee(s) {
    const n = committeeCount(s);
    if (n == null) return { title: "Committee not reported", line: "This node did not report its committee." };
    const epoch = s.bft.epoch != null ? `Epoch ${s.bft.epoch}. ` : "";
    return {
      title: `Committee: ${n} validators`,
      line: "Validators orbit the core. The node gives a count, not names.",
      detail: `${epoch}${n > MAX_DRAWN ? `Drawing ${MAX_DRAWN} of ${n}.` : "All drawn."} Source: node snapshot.`,
    };
  }

  function describeMining(s) {
    const m = s && s.mining;
    if (!m) return null;
    if (!m.active) return { title: "Mining not open yet", line: "Steel ring: no mining work is open on the chain." };
    const shares = m.shares != null && m.target_shares != null ? `Shares ${m.shares} of ${m.target_shares} this epoch. ` : "";
    return { title: `Mining open · epoch ${m.epoch}`, line: "Bronze ring: the chain is taking mining work.", detail: `${shares}Source: node snapshot.` };
  }

  // Keeps the canvas's accessible name in step with the figures on screen.
  function summary(s, rate) {
    if (!s) return "Network view: waiting for the node.";
    if (!s.reachable) return "Network view: no reply from the node at the last poll.";
    const n = committeeCount(s);
    const parts = [`chain ${s.chain.chain_id}, height ${Number(s.chain.height).toLocaleString()}`,
      n == null ? "committee not reported" : `committee ${n} validators`,
      rate == null ? "block rate not measured yet" : `block rate ${rate.toFixed(1)} per minute`];
    return `Network view: ${parts.join(", ")}.`;
  }

  // ------------------------------------------------------------ overlay ----

  // One real button per element, so every item is focusable and speaks its text.
  // Validators past MAX_DRAWN have no button; the committee button covers them.
  function buildEntries() {
    const make = (key) => {
      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "net-hit";
      btn.setAttribute("aria-describedby", "net-tip");
      const entry = { key, btn, x: 0, y: 0, on: false, item: null };
      btn.addEventListener("pointerenter", () => showTip(entry));
      btn.addEventListener("pointerleave", hideTip);
      btn.addEventListener("focus", () => showTip(entry));
      btn.addEventListener("blur", hideTip);
      btn.addEventListener("click", () => v.onSelect && entry.item && v.onSelect(entry.item));
      v.layer.append(btn);
      return entry;
    };
    v.entries = [make("core"), make("committee"), make("mining")];
    for (let i = 0; i < MAX_DRAWN; i += 1) v.entries.push(make(i));
  }

  function showTip(entry) {
    if (!entry.item || !entry.on) return;
    v.active = entry;
    v.tip.replaceChildren();
    const title = document.createElement("b");
    title.textContent = entry.item.title;
    const line = document.createElement("span");
    line.textContent = entry.item.line;
    v.tip.append(title, line);
    v.tip.hidden = false;
    entry.btn.setAttribute("aria-label", `${entry.item.title}. ${entry.item.line}`);
    placeTip(entry);
  }

  function placeTip(entry) {
    const below = entry.y < TIP_BELOW_PX;
    const lift = below ? "translate(-50%, 16px)" : "translate(-50%, calc(-100% - 16px))";
    v.tip.style.transform = `translate(${entry.x}px, ${entry.y}px) ${lift}`;
  }

  function hideTip() {
    v.active = null;
    v.tip.hidden = true;
  }

  function setEntry(entry, item, on) {
    entry.on = on;
    entry.item = item;
    entry.btn.hidden = !on;
    if (on) entry.btn.setAttribute("aria-label", `${item.title}. ${item.line}`);
  }

  // Text and visibility of every overlay button; runs on state change, not per frame.
  function labelEntries() {
    const s = v.snap;
    const [core, committee, mining] = v.entries;
    const n = committeeCount(s);
    setEntry(core, describeCore(s, v.rate), true);
    setEntry(committee, describeCommittee(s), true);
    const m = describeMining(s);
    setEntry(mining, m, m !== null);
    const drawn = n == null ? 0 : Math.min(n, MAX_DRAWN);
    v.entries.slice(3).forEach((entry, i) => {
      const on = i < drawn;
      setEntry(entry, on ? { title: `Validator ${i + 1} of ${n}`, line: "Anonymous committee member; the node reports only the count." } : null, on);
    });
    v.canvas.setAttribute("aria-label", summary(s, v.rate));
  }

  // Moves the buttons (and an open tooltip) to where the scene put each element.
  function syncEntries() {
    v.entries.forEach((entry) => {
      if (!entry.on) return;
      entry.btn.style.transform = `translate(${entry.x - HIT_PX / 2}px, ${entry.y - HIT_PX / 2}px)`;
    });
    if (v.active && v.active.on) placeTip(v.active);
  }

  // --------------------------------------------------------------- scene ----

  // Scene points are computed once per frame; the buttons read them from there.
  function scene(g) {
    const n = committeeCount(v.snap);
    const drawn = n == null ? 0 : Math.min(n, MAX_DRAWN);
    const spin = g.t * SPIN_PER_MS;
    g.spin = spin;
    g.pts = [];
    for (let i = 0; i < drawn; i += 1) g.pts.push(place((Math.PI * 2 * i) / drawn + g.t * ORBIT_PER_MS, 1, TILT, spin));
    g.live = !!(v.snap && v.snap.reachable);
    g.beat = v.reduced ? 1 : 1 + 0.06 * Math.sin(g.t / 500);
  }

  function ringPath(g, tilt, radius) {
    const { ctx } = g;
    ctx.beginPath();
    for (let i = 0; i <= SEGMENTS; i += 1) {
      const p = project(place((Math.PI * 2 * i) / SEGMENTS, radius, tilt, g.spin), g);
      if (i === 0) ctx.moveTo(p.x, p.y);
      else ctx.lineTo(p.x, p.y);
    }
    ctx.stroke();
  }

  function drawRings(g) {
    const { ctx } = g;
    ctx.strokeStyle = v.colors.grid;
    ctx.lineWidth = 1;
    ringPath(g, TILT, 1);
    ringPath(g, TILT + Math.PI / 2, 1);
    ringPath(g, TILT, 0.5);
  }

  function drawMiningRing(g) {
    const { ctx, s } = g;
    const m = s && s.mining;
    if (!m) return;
    ctx.save();
    ctx.strokeStyle = m.active ? v.colors.gold : v.colors.steel;
    ctx.lineWidth = m.active ? 2 : 1;
    ctx.setLineDash([5, 7]);
    ctx.lineDashOffset = v.reduced ? 0 : -g.t / 60;
    ctx.globalAlpha = m.active ? 0.9 : 0.45;
    ringPath(g, TILT, 0.5);
    ctx.restore();
  }

  function drawSpokes(g) {
    const { ctx, cx, cy } = g;
    ctx.strokeStyle = v.colors.teal;
    ctx.globalAlpha = 0.35;
    g.pts.forEach((p) => {
      const q = project(p, g);
      ctx.beginPath();
      ctx.moveTo(cx, cy);
      ctx.lineTo(q.x, q.y);
      ctx.stroke();
    });
    ctx.globalAlpha = 1;
  }

  // Each block: a bronze dot per validator, travelling from the core outward.
  // With no committee to reach, the block shows as one expanding ring instead.
  function drawPulses(g) {
    const { t } = g;
    for (let i = v.pulses.length - 1; i >= 0; i -= 1) {
      const p = v.pulses[i];
      const age = (t - p.t0) / p.dur;
      if (age >= 1) {
        v.pulses.splice(i, 1);
        v.lit = t;
      } else if (age >= 0) {
        drawPulse(g, age);
      }
    }
  }

  function drawPulse(g, age) {
    const { ctx, cx, cy, R } = g;
    ctx.fillStyle = v.colors.gold;
    ctx.shadowColor = v.colors.gold;
    ctx.shadowBlur = 10;
    ctx.globalAlpha = 1 - age * 0.5;
    if (!g.pts.length) {
      ctx.strokeStyle = v.colors.gold;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(cx, cy, R * (0.2 + age * 0.9), 0, Math.PI * 2);
      ctx.stroke();
    } else {
      g.pts.forEach((p) => {
        const q = project({ x: p.x * age, y: p.y * age, z: p.z * age }, g);
        ctx.beginPath();
        ctx.arc(q.x, q.y, 3.5, 0, Math.PI * 2);
        ctx.fill();
      });
    }
    ctx.shadowBlur = 0;
    ctx.globalAlpha = 1;
  }

  // Far validators first, so near ones paint over them.
  function drawOrbiters(g) {
    const { ctx } = g;
    const order = g.pts.map((p, i) => ({ i, q: project(p, g) })).sort((a, b) => b.q.z - a.q.z);
    // Lit: the last block's pulse just reached the committee.
    const lit = Math.max(0, 1 - (g.t - v.lit) / LIT_MS);
    ctx.fillStyle = v.colors.glow;
    ctx.shadowColor = lit > 0 ? v.colors.gold : v.colors.glow;
    ctx.shadowBlur = 12 + 18 * lit;
    order.forEach(({ i, q }) => {
      const depth = (q.z + 1) / 2;
      ctx.globalAlpha = 1 - 0.6 * Math.min(1, Math.max(0, depth));
      ctx.beginPath();
      ctx.arc(q.x, q.y, (5 + 3 * lit) * q.k, 0, Math.PI * 2);
      ctx.fill();
      v.entries[3 + i].x = q.x;
      v.entries[3 + i].y = q.y;
    });
    ctx.shadowBlur = 0;
    ctx.globalAlpha = 1;
  }

  // A lit sphere: a wide glow sized to the card, a core shaded from a
  // highlight to the deep accent, and on each new block a flash and a ring
  // that runs out past the orbit.
  function drawCore(g) {
    const { ctx, cx, cy, R, w, h, live, beat } = g;
    const flash = live ? Math.max(0, 1 - (g.t - v.flash) / FLASH_MS) : 0;
    const reach = Math.max(w, h) * 0.6;
    const glow = ctx.createRadialGradient(cx, cy, 0, cx, cy, reach);
    glow.addColorStop(0, live ? Charts.tint("--accent", 0.34 + 0.2 * flash) : Charts.tint("--warn", 0.16));
    glow.addColorStop(0.35, live ? Charts.tint("--accent", 0.1) : Charts.tint("--warn", 0.05));
    glow.addColorStop(1, Charts.tint("--accent", 0));
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, w, h);

    if (flash > 0) {
      ctx.strokeStyle = Charts.tint("--accent-hover", flash * 0.8);
      ctx.lineWidth = 2 + 2 * flash;
      ctx.beginPath();
      ctx.arc(cx, cy, R * (0.2 + 1.1 * (1 - flash)), 0, Math.PI * 2);
      ctx.stroke();
    }

    const r = R * 0.17 * beat * (1 + 0.25 * flash);
    const body = ctx.createRadialGradient(cx - r * 0.35, cy - r * 0.4, r * 0.1, cx, cy, r);
    body.addColorStop(0, live ? v.colors.glow : Charts.tint("--warn", 0.9));
    body.addColorStop(0.6, live ? v.colors.teal : Charts.tint("--warn", 0.55));
    body.addColorStop(1, live ? Charts.tint("--accent-grad-to", 0.9) : Charts.tint("--steel", 0.5));
    ctx.fillStyle = body;
    ctx.shadowColor = live ? v.colors.glow : v.colors.warn;
    ctx.shadowBlur = 24 + 20 * flash;
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.shadowBlur = 0;
  }

  // No reply: dim the view and draw a calm amber ring. A paused network is
  // waiting, not broken, so the ring is dashed and never red.
  function drawDown(g) {
    const { ctx, cx, cy, w, h } = g;
    ctx.fillStyle = Charts.tint("--bg", 0.35);
    ctx.fillRect(0, 0, w, h);
    ctx.strokeStyle = v.colors.warn;
    ctx.lineWidth = 2;
    ctx.setLineDash([6, 8]);
    ctx.beginPath();
    ctx.arc(cx, cy, Math.min(w, h) * 0.44, 0, Math.PI * 2);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  // Buttons for the core, committee ring and mining ring sit where those things are drawn.
  function placeFixed(g) {
    const [core, committee, mining] = v.entries;
    core.x = g.cx;
    core.y = g.cy;
    const top = place(-Math.PI / 2, 1, TILT, g.spin);
    const q = project(top, g);
    committee.x = q.x;
    committee.y = q.y;
    const m = project(place(-Math.PI / 2, 0.5, TILT, g.spin), g);
    mining.x = m.x;
    mining.y = m.y;
  }

  // ----------------------------------------------------------- public API ----

  // Token roles, read again whenever the theme changes (refresh).
  function readColors() {
    const css = Charts.css;
    return {
      teal: css("--accent"), glow: css("--accent-hover"), gold: css("--bronze"),
      steel: css("--steel"), warn: css("--warn"), grid: css("--chart-grid"),
    };
  }

  function refresh() {
    v.colors = readColors();
    frame(performance.now());
  }

  function init({ canvas, layer, tip, reduced, onSelect }) {
    Object.assign(v, { canvas, layer, tip, reduced, onSelect, colors: readColors() });
    buildEntries();
    labelEntries();
    window.addEventListener("resize", () => frame(performance.now()));
  }

  // Called when the snapshot or block rate changes. A new block starts pulses.
  function update(snap, rate, newBlock) {
    v.snap = snap;
    v.rate = rate;
    if (newBlock && !v.reduced) {
      v.flash = performance.now();
      v.pulses.push({ t0: v.flash, dur: travelMs(rate) });
    }
    labelEntries();
  }

  // Draws one frame and syncs the buttons. Reduced motion calls this only on change.
  function frame(t) {
    const f = Charts.fit(v.canvas);
    if (!f) return;
    const g = { ctx: f.ctx, w: f.w, h: f.h, cx: f.w / 2, cy: f.h / 2, R: Math.min(f.w, f.h) * ORBIT_FRAC, s: v.snap, t: v.reduced ? 0 : t };
    scene(g);
    drawRings(g);
    drawMiningRing(g);
    drawSpokes(g);
    drawOrbiters(g);
    drawPulses(g);
    drawCore(g);
    if (!g.live) drawDown(g);
    placeFixed(g);
    syncEntries();
  }

  return { init, update, frame, refresh };
})();
