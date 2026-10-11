// Canvas drawing for the hub. No libraries: the app runs offline and its CSP
// allows only its own scripts. Every function draws what it is given; none
// invents a value.
"use strict";

const Charts = (() => {
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

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
    const r = Math.min(w * 0.4, h * 0.4);
    const cy = h * 0.5 + r * 0.12;
    const start = Math.PI * 0.8;
    const span = Math.PI * 1.4;
    const frac = peak > 0 && hps ? Math.min(1, hps / peak) : 0;

    ctx.lineCap = "round";
    ctx.lineWidth = r * 0.12;
    ctx.strokeStyle = css("--track");
    ctx.beginPath();
    ctx.arc(cx, cy, r, start, start + span);
    ctx.stroke();

    for (let i = 0; i <= 28; i += 1) {
      const a = start + (span * i) / 28;
      const inner = r * (i % 7 === 0 ? 0.74 : 0.8);
      ctx.strokeStyle = i / 28 <= frac ? css("--teal-glow") : css("--track");
      ctx.lineWidth = i % 7 === 0 ? 2 : 1;
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(a) * inner, cy + Math.sin(a) * inner);
      ctx.lineTo(cx + Math.cos(a) * r * 0.84, cy + Math.sin(a) * r * 0.84);
      ctx.stroke();
    }

    if (frac > 0) {
      const grad = ctx.createLinearGradient(cx - r, cy, cx + r, cy);
      grad.addColorStop(0, css("--teal"));
      grad.addColorStop(1, css("--gold"));
      ctx.lineWidth = r * 0.12;
      ctx.strokeStyle = grad;
      ctx.shadowColor = css("--teal-glow");
      ctx.shadowBlur = active ? 18 + 8 * Math.sin(t / 400) : 6;
      ctx.beginPath();
      ctx.arc(cx, cy, r, start, start + span * frac);
      ctx.stroke();
      ctx.shadowBlur = 0;
      const a = start + span * frac;
      ctx.fillStyle = "#fff";
      ctx.beginPath();
      ctx.arc(cx + Math.cos(a) * r, cy + Math.sin(a) * r, r * 0.05, 0, Math.PI * 2);
      ctx.fill();
    }

    if (active) {
      const a = (t / 1400) % (Math.PI * 2);
      const sheen = ctx.createConicGradient(a, cx, cy);
      sheen.addColorStop(0, "rgba(126,234,240,0.0)");
      sheen.addColorStop(0.08, "rgba(126,234,240,0.16)");
      sheen.addColorStop(0.16, "rgba(126,234,240,0.0)");
      ctx.fillStyle = sheen;
      ctx.beginPath();
      ctx.arc(cx, cy, r * 0.68, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  // A smoothed area chart of [{t_ms, hps}], newest on the right, with the
  // average drawn as a dashed line.
  function area(canvas, samples, { color = "--teal", label = rate, floor = false } = {}) {
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx, w, h } = fitted;
    const pad = { l: 8, r: 8, t: 14, b: 18 };
    ctx.font = `11px ${css("--mono")}`;
    if (!samples.length) {
      ctx.fillStyle = css("--dim");
      ctx.textAlign = "center";
      ctx.fillText("No samples yet: start the miner", w / 2, h / 2);
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

    ctx.strokeStyle = css("--grid");
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
    grad.addColorStop(0, "rgba(31,181,194,0.45)");
    grad.addColorStop(1, "rgba(31,181,194,0.0)");
    ctx.fillStyle = grad;
    ctx.fill(fill);
    ctx.strokeStyle = css(color);
    ctx.lineWidth = 2;
    ctx.shadowColor = css("--teal-glow");
    ctx.shadowBlur = 10;
    ctx.stroke(path);
    ctx.shadowBlur = 0;

    const avg = values.reduce((a, b) => a + b, 0) / values.length;
    ctx.setLineDash([4, 5]);
    ctx.strokeStyle = css("--gold");
    ctx.beginPath();
    ctx.moveTo(pad.l, y(avg));
    ctx.lineTo(w - pad.r, y(avg));
    ctx.stroke();
    ctx.setLineDash([]);
    const a = label(avg);
    ctx.fillStyle = css("--gold");
    ctx.textAlign = "right";
    ctx.fillText(`avg ${a.value} ${a.unit}`, w - pad.r, y(avg) - 5);

    const last = samples[samples.length - 1];
    ctx.fillStyle = "#fff";
    ctx.beginPath();
    ctx.arc(x(last.t_ms), y(last.hps), 3.5, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = css("--dim");
    ctx.textAlign = "left";
    const mins = Math.round((t1 - t0) / 60000);
    ctx.fillText(mins > 0 ? `last ${mins} min` : "last minute", pad.l, h - 4);
  }

  // A ring meter: value/max, coloured from teal to gold to red past `warn`.
  function ring(canvas, { value, max, warn, text }) {
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx, w, h } = fitted;
    const cx = w / 2;
    const cy = h / 2;
    const r = Math.min(w, h) / 2 - 6;
    ctx.lineWidth = 6;
    ctx.lineCap = "round";
    ctx.strokeStyle = css("--track");
    ctx.beginPath();
    ctx.arc(cx, cy, r, 0, Math.PI * 2);
    ctx.stroke();
    if (value != null) {
      const frac = Math.max(0, Math.min(1, value / max));
      ctx.strokeStyle = warn != null && value >= warn ? css("--hot") : frac > 0.6 ? css("--gold") : css("--teal");
      ctx.shadowColor = ctx.strokeStyle;
      ctx.shadowBlur = 8;
      ctx.beginPath();
      ctx.arc(cx, cy, r, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * frac);
      ctx.stroke();
      ctx.shadowBlur = 0;
    }
    ctx.fillStyle = css("--text");
    ctx.font = `600 13px ${css("--display")}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillText(value == null ? "—" : text, cx, cy);
  }

  // Particles that burst from a point when a share lands. Purely decorative,
  // and only ever triggered by a real accepted share.
  const bursts = [];
  function burst(x, y, gold) {
    for (let i = 0; i < 26; i += 1) {
      const a = Math.random() * Math.PI * 2;
      const s = 1.2 + Math.random() * 3.2;
      bursts.push({ x, y, vx: Math.cos(a) * s, vy: Math.sin(a) * s - 1, life: 1, gold });
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
      ctx.fillStyle = p.gold ? css("--gold") : css("--teal-glow");
      ctx.beginPath();
      ctx.arc(p.x, p.y, 2.2 * p.life + 0.6, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
  }

  // The overview orb: committee members orbit a glowing core; each new block
  // sends a ripple outward. `members` and `ripples` come from real RPC data.
  function orb(canvas, { members, ripples, live, t }) {
    const fitted = fit(canvas);
    if (!fitted) return;
    const { ctx, w, h } = fitted;
    const cx = w / 2;
    const cy = h / 2;
    const r = Math.min(w, h) * 0.32;

    const glow = ctx.createRadialGradient(cx, cy, 0, cx, cy, r * 1.6);
    glow.addColorStop(0, live ? "rgba(31,181,194,0.35)" : "rgba(147,164,170,0.18)");
    glow.addColorStop(1, "rgba(5,11,15,0)");
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, w, h);

    for (let i = ripples.length - 1; i >= 0; i -= 1) {
      const age = (t - ripples[i]) / 2600;
      if (age >= 1) {
        ripples.splice(i, 1);
        continue;
      }
      ctx.strokeStyle = `rgba(201,168,106,${(1 - age) * 0.7})`;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(cx, cy, r * 0.25 + age * r * 1.4, 0, Math.PI * 2);
      ctx.stroke();
    }

    ctx.strokeStyle = css("--grid");
    ctx.lineWidth = 1;
    [0.62, 1].forEach((k) => {
      ctx.beginPath();
      ctx.ellipse(cx, cy, r * k, r * k * 0.42, -0.2, 0, Math.PI * 2);
      ctx.stroke();
    });

    const n = Math.max(members, 0);
    const pts = [];
    for (let i = 0; i < n; i += 1) {
      const a = (Math.PI * 2 * i) / n + t / 6000;
      pts.push([cx + Math.cos(a) * r, cy + Math.sin(a) * r * 0.42 * Math.cos(0.2) + Math.cos(a) * r * Math.sin(-0.2) * 0.2]);
    }
    ctx.strokeStyle = "rgba(31,181,194,0.25)";
    pts.forEach((p, i) =>
      pts.slice(i + 1).forEach((q) => {
        ctx.beginPath();
        ctx.moveTo(p[0], p[1]);
        ctx.lineTo(q[0], q[1]);
        ctx.stroke();
      }),
    );
    pts.forEach(([x, y]) => {
      ctx.fillStyle = css("--teal-glow");
      ctx.shadowColor = css("--teal-glow");
      ctx.shadowBlur = 14;
      ctx.beginPath();
      ctx.arc(x, y, 6, 0, Math.PI * 2);
      ctx.fill();
    });
    ctx.shadowBlur = 0;

    const pulse = 1 + 0.06 * Math.sin(t / 500);
    const core = ctx.createRadialGradient(cx, cy, 0, cx, cy, r * 0.22 * pulse);
    core.addColorStop(0, "#ffffff");
    core.addColorStop(0.35, live ? css("--teal-glow") : css("--dim"));
    core.addColorStop(1, "rgba(31,181,194,0)");
    ctx.fillStyle = core;
    ctx.beginPath();
    ctx.arc(cx, cy, r * 0.22 * pulse, 0, Math.PI * 2);
    ctx.fill();
  }

  return { fit, rate, gauge, area, ring, burst, drawBursts, orb };
})();
