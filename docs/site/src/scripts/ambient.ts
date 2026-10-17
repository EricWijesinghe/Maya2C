// The living background: one WebGL context for the whole site, drawing the
// motif the route selected (html[data-motif], set before first paint in
// Head.astro). Phones, reduced motion and browsers without WebGL never reach
// here: the CSS fallback for each motif (maya-living.css) stays on screen.
import { FIELD_FS, FIELD_VS, POINT_FS, POINT_VS } from "./ambient-shaders";

const PHONE_PX = 760;
const DPR_CAP = 1.5;
const POINT_COUNT = 170;
const POINT_MOTIFS = new Set(["lattice", "sunrise"]);
const FIELD_CODE: Record<string, number> = { aurora: 2, blueprint: 3, circuit: 4 };
// Full-screen fields are costed per pixel. The aurora is soft, so it is drawn at
// half resolution and the browser upscales it. Fields run at 30 fps: their
// motion is slow, and a 60 fps full-screen pass is what took the guides pages
// from 98 to 82 in the lab (software GL, desktop preset).
const FIELD_SCALE: Record<string, number> = { aurora: 0.5, blueprint: 1, circuit: 1 };
const FIELD_MIN_GAP_MS = 1000 / 30;

type Uniforms = Record<string, WebGLUniformLocation | null>;

function compile(gl: WebGLRenderingContext, type: number, src: string): WebGLShader | null {
  const shader = gl.createShader(type);
  if (!shader) return null;
  gl.shaderSource(shader, src);
  gl.compileShader(shader);
  if (gl.getShaderParameter(shader, gl.COMPILE_STATUS)) return shader;
  gl.deleteShader(shader);
  return null;
}

function link(gl: WebGLRenderingContext, vsSrc: string, fsSrc: string, names: string[]): { prog: WebGLProgram; u: Uniforms } | null {
  const vs = compile(gl, gl.VERTEX_SHADER, vsSrc);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fsSrc);
  const prog = gl.createProgram();
  if (!vs || !fs || !prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return null;
  gl.useProgram(prog);
  const u: Uniforms = {};
  for (const name of names) u[name] = gl.getUniformLocation(prog, name);
  return { prog, u };
}

// Section tokens are hex in maya-living.css; the shaders take 0..1 floats.
function tokenRgb(css: CSSStyleDeclaration, name: string, fallback: string): [number, number, number] {
  const hex = (css.getPropertyValue(name).trim() || fallback).replace("#", "");
  const n = parseInt(hex, 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

function seedPoints(): Float32Array {
  // Per point: x, y in [-1,1]; depth z in [0.25,1.6]; palette index 0..2.
  const seed = new Float32Array(POINT_COUNT * 4);
  for (let i = 0; i < POINT_COUNT; i++) {
    seed[i * 4] = Math.random() * 2 - 1;
    seed[i * 4 + 1] = Math.random() * 2 - 1;
    seed[i * 4 + 2] = 0.25 + Math.random() * 1.35;
    seed[i * 4 + 3] = Math.floor(Math.random() * 3);
  }
  return seed;
}

export function startAmbient(): void {
  const root = document.documentElement;
  const canvas = document.getElementById("ambient-canvas") as HTMLCanvasElement | null;
  if (!canvas) return;
  if (window.matchMedia("(prefers-reduced-motion: reduce)").matches || window.innerWidth < PHONE_PX) return;
  const gl = canvas.getContext("webgl", { antialias: false, alpha: true, premultipliedAlpha: true }) as WebGLRenderingContext | null;
  if (!gl) return;

  const motif = root.dataset.motif ?? "lattice";
  const points = POINT_MOTIFS.has(motif);
  // Every uniform name for both programs: a name a program does not declare
  // resolves to null, and uniform calls on null are no-ops.
  const names = ["uRes", "uDpr", "uT", "uS", "uSpeed", "uAlpha", "uSunrise", "uMotif", "uA", "uB", "uC"];
  const prog = points ? link(gl, POINT_VS, POINT_FS, names) : link(gl, FIELD_VS, FIELD_FS, names);
  if (!prog) return;
  const { u } = prog;

  const css = getComputedStyle(root);
  const [ar, ag, ab] = tokenRgb(css, "--acc", "#1fb5c2");
  const [br, bg, bb] = tokenRgb(css, "--acc-2", "#c9a86a");
  const [cr, cg, cb] = tokenRgb(css, "--acc-3", "#7eeaf0");
  gl.uniform3f(u.uA, ar, ag, ab);
  gl.uniform3f(u.uB, br, bg, bb);
  gl.uniform3f(u.uC, cr, cg, cb);
  gl.uniform1f(u.uSpeed, Number(css.getPropertyValue("--speed").trim()) || 1);
  // The light theme paints the same field at lower strength so text stays readable.
  gl.uniform1f(u.uAlpha, Number(css.getPropertyValue("--field-alpha").trim()) || 1);
  gl.uniform1f(u.uSunrise, motif === "sunrise" ? 1 : 0);
  gl.uniform1f(u.uMotif, FIELD_CODE[motif] ?? 3);

  const data = points ? seedPoints() : new Float32Array([-1, -1, 3, -1, -1, 3]);
  const size = points ? 4 : 2;
  const count = data.length / size;
  gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
  gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(prog.prog, "a");
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
  gl.enable(gl.BLEND);
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);

  // Pixel ratio capped: the fields are soft, so 1.5 is plenty. uDpr is canvas
  // pixels per CSS pixel, so the shaders stay correct at the reduced scale.
  const scale = points ? 1 : (FIELD_SCALE[motif] ?? 1);
  const dpr = Math.min(window.devicePixelRatio || 1, DPR_CAP) * scale;
  const resize = () => {
    const w = Math.floor(window.innerWidth * dpr);
    const h = Math.floor(window.innerHeight * dpr);
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
      gl.viewport(0, 0, w, h);
    }
    gl.uniform2f(u.uRes, w, h);
    gl.uniform1f(u.uDpr, dpr);
  };
  resize();
  window.addEventListener("resize", resize);

  let scroll = 0;
  window.addEventListener("scroll", () => {
    scroll = window.scrollY / Math.max(1, window.innerHeight);
  }, { passive: true });

  const t0 = performance.now();
  const minGap = points ? 0 : FIELD_MIN_GAP_MS;
  let last = -Infinity;
  let raf = 0;
  const frame = (now: number) => {
    raf = requestAnimationFrame(frame);
    if (now - last < minGap) return;
    last = now;
    gl.uniform1f(u.uT, (now - t0) / 1000);
    gl.uniform1f(u.uS, scroll);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);
    gl.drawArrays(points ? gl.POINTS : gl.TRIANGLES, 0, count);
  };
  // One loop at a time: hiding and showing the tab quickly must not stack loops.
  const start = () => { if (!raf && !document.hidden) raf = requestAnimationFrame(frame); };
  const stop = () => { cancelAnimationFrame(raf); raf = 0; };
  document.addEventListener("visibilitychange", () => (document.hidden ? stop() : start()));

  // A lost context hands the page back to the CSS fallback instead of a blank field.
  canvas.addEventListener("webglcontextlost", (e) => {
    e.preventDefault();
    stop();
    root.classList.remove("ambient-gl");
  });

  root.classList.add("ambient-gl");
  start();
}
