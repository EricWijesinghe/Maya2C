// Canvas drawing for the hub. No libraries: the app runs offline and its CSP
// allows only its own scripts. Every function draws what it is given; none
// invents a value. Colours are token roles (tokens.css), read on every draw,
// so a theme change shows on the next frame or redraw.
"use strict";

const Charts = (() => {
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  const probe = document.createElement("canvas").getContext("2d");

  // A token at an alpha. The canvas parses the token (hex or rgb()), so the
  // role stays the single source of the colour.
  function tint(name, alpha) {
    probe.fillStyle = "transparent";
    probe.fillStyle = css(name) || "transparent";
    const v = probe.fillStyle;
    if (v.startsWith("#")) {
      const n = parseInt(v.slice(1), 16);
      return `rgb(${(n >> 16) & 255} ${(n >> 8) & 255} ${n & 255} / ${alpha})`;
    }
    const [r, g, b, a = 1] = (v.match(/[\d.]+/g) || [0, 0, 0, 0]).map(Number);
    return `rgb(${r} ${g} ${b} / ${alpha * a})`;
  }

  // Keeps a canvas's accessible name in step with what it draws.
  function describe(canvas, text) {
    if (!canvas) return;
    if (canvas.getAttribute("role") !== "img") canvas.setAttribute("role", "img");
    if (canvas.getAttribute("aria-label") !== text) canvas.setAttribute("aria-label", text);
  }

  // Sizes a canvas to its box at the device pixel ratio; returns the context
  // scaled so callers draw in CSS pixels.
  function fit(canvas) {
    const dpr = window.devicePixelRatio || 1;
    const { width, height } = canvas.getBoundingClientRect();
    // A canvas on a hidden page measures zero; drawing there would only
    // throw on negative radii.
    if (width < 4 || height < 4) return null;
    const w = Math.max(1, Math.round(width * dpr));
    const h = Math.max(1, Math.round(height * dpr));
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);
    return { ctx, w: width, h: height };
  }

  // Hash rate with a unit a person can read.
  function rate(hps) {
    if (hps == null || !isFinite(hps)) return { value: "—", unit: "H/s" };
    const units = ["H/s", "kH/s", "MH/s", "GH/s", "TH/s"];
    let i = 0;
    let v = hps;
    while (v >= 1000 && i < units.length - 1) {
      v /= 1000;
      i += 1;
    }
    return { value: v >= 100 ? v.toFixed(0) : v.toFixed(1), unit: units[i] };
  }

  // The hero gauge: an arc whose sweep follows the rate against the session
  // peak, a rotating sheen while mining, and tick marks.
  function gauge(canvas, { hps, peak, active, t }) {
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx, w, h } = fitted;
    const cx = w / 2;
    const r = Math.min(w * 0.42, h * 0.42);
    const cy = h * 0.5 + r * 0.12;
    const start = Math.PI * 0.8;
    const span = Math.PI * 1.4;
    const frac = peak > 0 && hps ? Math.min(1, hps / peak) : 0;

    // A soft halo behind the dial; brighter while mining.
    const halo = ctx.createRadialGradient(cx, cy, r * 0.2, cx, cy, r * 1.25);
    halo.addColorStop(0, tint("--accent", active ? 0.2 : 0.08));
    halo.addColorStop(1, tint("--accent", 0));
    ctx.fillStyle = halo;
    ctx.fillRect(0, 0, w, h);

    ctx.lineCap = "round";
    ctx.lineWidth = r * 0.12;
    ctx.strokeStyle = tint("--text-3", 0.22);
    ctx.beginPath();
    ctx.arc(cx, cy, r, start, start + span);
    ctx.stroke();

    for (let i = 0; i <= 28; i += 1) {
      const a = start + (span * i) / 28;
      const inner = r * (i % 7 === 0 ? 0.74 : 0.8);
      ctx.strokeStyle = i / 28 <= frac && frac > 0 ? css("--accent-hover") : tint("--text-3", 0.35);
      ctx.lineWidth = i % 7 === 0 ? 2 : 1;
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(a) * inner, cy + Math.sin(a) * inner);
      ctx.lineTo(cx + Math.cos(a) * r * 0.84, cy + Math.sin(a) * r * 0.84);
      ctx.stroke();
    }

    if (frac > 0) {
      const grad = ctx.createLinearGradient(cx - r, cy, cx + r, cy);
      grad.addColorStop(0, css("--accent-grad-to"));
      grad.addColorStop(0.6, css("--accent"));
      grad.addColorStop(1, css("--bronze"));
      ctx.lineWidth = r * 0.12;
      ctx.strokeStyle = grad;
      ctx.shadowColor = tint("--accent", 0.8);
      ctx.shadowBlur = active ? 18 + 8 * Math.sin(t / 400) : 6;
      ctx.beginPath();
      ctx.arc(cx, cy, r, start, start + span * frac);
      ctx.stroke();
      ctx.shadowBlur = 0;
      const a = start + span * frac;
      ctx.fillStyle = css("--text");
      ctx.strokeStyle = css("--surface-1");
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(cx + Math.cos(a) * r, cy + Math.sin(a) * r, r * 0.055, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
    }

    if (active) {
      const a = (t / 1400) % (Math.PI * 2);
      const sheen = ctx.createConicGradient(a, cx, cy);
      sheen.addColorStop(0, tint("--accent-hover", 0));
      sheen.addColorStop(0.08, tint("--accent-hover", 0.16));
      sheen.addColorStop(0.16, tint("--accent-hover", 0));
      ctx.fillStyle = sheen;
      ctx.beginPath();
      ctx.arc(cx, cy, r * 0.68, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  // A smoothed area chart of [{t_ms, hps}], newest on the right, with the
  // average drawn as a dashed line. `name` is the chart's spoken name.
  function area(canvas, samples, { color = "--chart-1", label = rate, floor = false, name = "Chart", empty = "No samples yet" } = {}) {
    if (!samples.length) describe(canvas, `${name}: ${empty.toLowerCase()}`);
    else {
      // Named before drawing, so a chart on a hidden page still says what it holds.
      const values = samples.map((s) => s.hps);
      const avg = label(values.reduce((x, y) => x + y, 0) / values.length);
      const now = label(values[values.length - 1]);
      const mins = Math.round((samples[samples.length - 1].t_ms - samples[0].t_ms) / 60000);
      describe(canvas, `${name}: latest ${now.value} ${now.unit}, average ${avg.value} ${avg.unit}, ${mins > 0 ? `last ${mins} min` : "last minute"}`.replace(/ ,/g, ","));
    }
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx, w, h } = fitted;
    const pad = { l: 8, r: 8, t: 14, b: 18 };
    ctx.font = `11px ${css("--font-mono")}`;
    if (!samples.length) {
      ctx.fillStyle = css("--text-2");
      ctx.textAlign = "center";
      ctx.fillText(empty, w / 2, h / 2);
      return;
    }
    const values = samples.map((s) => s.hps);
    // `floor` charts a counter (block height) against its own range, not zero.
    const min = floor ? Math.min(...values) : 0;
    const max = (floor ? Math.max(...values) + 1 : Math.max(...values) * 1.15) || 1;
    const t0 = samples[0].t_ms;
    const t1 = samples[samples.length - 1].t_ms;
    const x = (t) => pad.l + ((t - t0) / Math.max(1, t1 - t0)) * (w - pad.l - pad.r);
    const y = (v) => h - pad.b - ((v - min) / (max - min || 1)) * (h - pad.t - pad.b);

    ctx.strokeStyle = css("--chart-grid");
    ctx.lineWidth = 1;
    for (let i = 1; i <= 3; i += 1) {
      const gy = pad.t + ((h - pad.t - pad.b) * i) / 4;
      ctx.beginPath();
      ctx.moveTo(pad.l, gy);
      ctx.lineTo(w - pad.r, gy);
      ctx.stroke();
    }

    const path = new Path2D();
    samples.forEach((s, i) => {
      const px = x(s.t_ms);
      const py = y(s.hps);
      if (i === 0) path.moveTo(px, py);
      else {
        const prev = samples[i - 1];
        const mx = (x(prev.t_ms) + px) / 2;
        path.bezierCurveTo(mx, y(prev.hps), mx, py, px, py);
      }
    });
    const fill = new Path2D(path);
    fill.lineTo(x(t1), h - pad.b);
    fill.lineTo(x(t0), h - pad.b);
    fill.closePath();
    const grad = ctx.createLinearGradient(0, pad.t, 0, h - pad.b);
    grad.addColorStop(0, tint(color, 0.38));
    grad.addColorStop(1, tint(color, 0));
    ctx.fillStyle = grad;
    ctx.fill(fill);
    ctx.strokeStyle = css(color);
    ctx.lineWidth = 2;
    ctx.shadowColor = tint(color, 0.6);
    ctx.shadowBlur = 10;
    ctx.stroke(path);
    ctx.shadowBlur = 0;

    const avg = values.reduce((a, b) => a + b, 0) / values.length;
    ctx.setLineDash([4, 5]);
    ctx.strokeStyle = css("--bronze");
    ctx.beginPath();
    ctx.moveTo(pad.l, y(avg));
    ctx.lineTo(w - pad.r, y(avg));
    ctx.stroke();
    ctx.setLineDash([]);
    const a = label(avg);
    ctx.fillStyle = css("--text-2");
    ctx.textAlign = "right";
    ctx.fillText(`avg ${a.value} ${a.unit}`, w - pad.r, y(avg) - 5);

    const last = samples[samples.length - 1];
    ctx.fillStyle = css(color);
    ctx.strokeStyle = css("--surface-1");
    ctx.lineWidth = 2;
    ctx.beginPath();
    ctx.arc(x(last.t_ms), y(last.hps), 4, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = css("--text-2");
    ctx.textAlign = "left";
    const mins = Math.round((t1 - t0) / 60000);
    const span = mins > 0 ? `last ${mins} min` : "last minute";
    ctx.fillText(span, pad.l, h - 4);
  }

  // A ring meter: value/max, teal, bronze past 60 %, red past `warn`.
  function ring(canvas, { value, max, warn, text, name = "Meter", spoken }) {
    describe(canvas, value == null ? `${name}: not reported` : `${name}: ${spoken || text}`);
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx, w, h } = fitted;
    const cx = w / 2;
    const cy = h / 2;
    const r = Math.min(w, h) / 2 - 6;
    ctx.lineWidth = 6;
    ctx.lineCap = "round";
    ctx.strokeStyle = tint("--text-3", 0.22);
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.stroke();
    if (value != null) {
      const frac = Math.max(0, Math.min(1, value / max));
      const role = warn != null && value >= warn ? "--danger" : frac > 0.6 ? "--bronze" : "--accent";
      ctx.strokeStyle = css(role);
      ctx.shadowColor = tint(role, 0.7);
      ctx.shadowBlur = 8;
      ctx.beginPath();
      ctx.arc(cx, cy, r, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * frac);
      ctx.stroke();
      ctx.shadowBlur = 0;
    }
    ctx.fillStyle = css("--text");
    ctx.font = `600 13px ${css("--hub-display")}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(value == null ? "—" : text, cx, cy);
  }

  // Sparks that burst from a point when a share lands. Purely decorative,
  // and only ever triggered by a real accepted share.
  const bursts = [];
  function burst(x, y) {
    for (let i = 0; i < 26; i += 1) {
      const a = Math.random() * Math.PI * 2;
      const s = 1.2 + Math.random() * 3.2;
      bursts.push({ x, y, vx: Math.cos(a) * s, vy: Math.sin(a) * s - 1, life: 1, bronze: i % 3 !== 0 });
    }
  }
  function drawBursts(canvas) {
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx } = fitted;
    for (let i = bursts.length - 1; i >= 0; i -= 1) {
      const p = bursts[i];
      p.x += p.vx;
      p.y += p.vy;
      p.vy += 0.06;
      p.life -= 0.018;
      if (p.life <= 0) {
        bursts.splice(i, 1);
        continue;
      }
      ctx.globalAlpha = p.life;
      ctx.fillStyle = css(p.bronze ? "--bronze" : "--accent-hover");
      ctx.beginPath();
      ctx.arc(p.x, p.y, 2.2 * p.life + 0.6, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  }

  return { fit, rate, gauge, area, ring, burst, drawBursts, describe, tint, css };
})();
