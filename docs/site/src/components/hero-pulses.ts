// One pulse along a grid row of the hero for each REAL finalized block from the
// explorer's WebSocket; nothing is simulated. Hero.astro imports this module
// only while the testnet runs, so a paused page never downloads a canvas script.
import { onBlocks, fmt } from "../scripts/live";
import { whenIdle } from "../scripts/idle";

const GRID = 24; // px, matches .dots background-size in Hero.astro
const LIFE_MS = 1600;
const TAIL_PX = 96;

export function mountPulses(hero: HTMLElement): void {
  const canvas = hero.querySelector<HTMLCanvasElement>(".pulses");
  const ctx = canvas?.getContext("2d");
  if (!canvas || !ctx) return;
  const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const pulses: { y: number; x0: number; dir: number; t0: number }[] = [];
  let running = false;

  const size = () => {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    canvas.width = canvas.clientWidth * dpr;
    canvas.height = canvas.clientHeight * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  };
  const frame = (now: number) => {
    const w = canvas.clientWidth;
    ctx.clearRect(0, 0, w, canvas.clientHeight);
    const color = getComputedStyle(hero).getPropertyValue("--accent").trim();
    for (let i = pulses.length - 1; i >= 0; i--) {
      const p = pulses[i], k = (now - p.t0) / LIFE_MS;
      if (k >= 1) { pulses.splice(i, 1); continue; }
      const head = p.x0 + p.dir * k * w * 0.6, tail = head - p.dir * TAIL_PX;
      const g = ctx.createLinearGradient(tail, 0, head, 0);
      g.addColorStop(0, "transparent");
      g.addColorStop(1, color);
      ctx.globalAlpha = Math.sin(Math.PI * k) * 0.9;
      ctx.strokeStyle = g;
      ctx.lineWidth = 1.5;
      ctx.beginPath(); ctx.moveTo(tail, p.y); ctx.lineTo(head, p.y); ctx.stroke();
    }
    ctx.globalAlpha = 1;
    if (pulses.length) requestAnimationFrame(frame); else running = false;
  };
  const pulse = () => {
    if (still || document.hidden) return;
    const rows = Math.max(1, Math.floor(canvas.clientHeight / GRID));
    const dir = Math.random() < 0.5 ? 1 : -1;
    pulses.push({ y: (1 + Math.floor(Math.random() * (rows - 1))) * GRID + 0.5, x0: dir > 0 ? 0 : canvas.clientWidth, dir, t0: performance.now() });
    if (!running) { running = true; requestAnimationFrame(frame); }
  };

  const stateEl = hero.querySelector("[data-hero-state]");
  const tipEl = hero.querySelector<HTMLElement>("[data-hero-tip]");
  const heightEl = hero.querySelector("[data-hero-height]");
  let seen = 0;
  whenIdle(() => {
    size();
    new ResizeObserver(size).observe(canvas);
    onBlocks((b) => {
      if (b.height <= seen) return;
      seen = b.height;
      if (heightEl) heightEl.textContent = fmt(b.height);
      if (tipEl) tipEl.hidden = false;
      pulse();
    }, (live) => {
      if (stateEl) stateEl.textContent = live ? "finalizing" : "reconnecting…";
      hero.classList.toggle("live", live);
    });
  });
}
