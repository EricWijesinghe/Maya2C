// Pure model of the home-page propagation scene. Everything on screen is a
// function of simulated time t, so the loop cannot drift and the static poster
// is the same model drawn once. No randomness: a flat lattice on a plane, a
// spatial wavefront, and nearest-grid edges, so every visit shows the same block.
//
// The scene illustrates the protocol; it is not a measurement. The validator
// stride and the 2/3 quorum are picked for legibility, and the page says so.

export const PERIOD = 10; // seconds per block cycle
const COLS = 28;
const ROWS = 12;
const SPACING = 0.5;
const FRONT_START = 0.9; // the core is born in [0, FRONT_START)
const FRONT_DUR = 3.2; // seconds for the wave to cross the whole field
const OVERSHOOT_C1 = 1; // easeOutBack amount: the crest overshoots the field edge by ~4%
const SHELL_W = 0.55; // thickness of the displaced shell, in field units
const SHELL_AMP = 0.26; // how far the lattice is lifted under the shell
const RISE = 0.3; // seconds for one node to light
const BEAM_TRAVEL = 1.8;
const SLH_GAP = 0.3;
const VALIDATOR_EVERY_COL = 4;
const VALIDATOR_EVERY_ROW = 3;
const HOLD = 1.0; // the lock is held for a beat before the camera eases out
const WIDE = 20; // camera distance at rest
const PUSH_DEPTH = 4.5; // the push-in at the impact, in field units
const CLOCK_SHAKE = 0.05;

export const PHASES = ["proposed", "propagated", "signed twice (ML-DSA + SLH-DSA)", "final"] as const;

export type Vec3 = readonly [number, number, number];

export type Model = {
  points: Vec3[];
  edges: [number, number][];
  core: number;
  dist: number[]; // planar distance from the core, field units
  maxDist: number;
  validators: number[];
  quorum: number;
  ignite: number[]; // per node: time the wavefront reaches it
  mlStart: number[];
  slhStart: number[];
  travel: number;
  signStart: number;
  finalAt: number;
};

const easeOut = (x: number) => 1 - (1 - clamp01(x)) ** 3;
const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
// Ease-in-out: slow start and slow finish. Heavy, never linear.
export const ease = (x: number) => { const y = clamp01(x); return y < 0.5 ? 4 * y * y * y : 1 - (-2 * y + 2) ** 3 / 2; };

// easeOutBack with momentum: the crest runs slightly past its target, then settles.
const OB_C3 = OVERSHOOT_C1 + 1;
const backOut = (p: number) => 1 + OB_C3 * (p - 1) ** 3 + OVERSHOOT_C1 * (p - 1) ** 2;
const PEAK = 1 - (2 * OVERSHOOT_C1) / (3 * OB_C3); // backOut is monotone up to here

// Wavefront radius (field units) at time t; zero before the front starts.
export function frontRadius(m: Model, t: number): number {
  if (t < FRONT_START) return 0;
  return m.maxDist * backOut(Math.min((t - FRONT_START) / FRONT_DUR, 1));
}

// Inverse of backOut on the monotone part: the front time that reaches distance d.
function frontTimeFor(d: number, maxDist: number): number {
  const target = d / maxDist;
  let lo = 0, hi = PEAK;
  for (let k = 0; k < 60; k++) {
    const mid = (lo + hi) / 2;
    if (backOut(mid) < target) lo = mid; else hi = mid;
  }
  return FRONT_START + FRONT_DUR * hi;
}

function lattice(): Vec3[] {
  return Array.from({ length: COLS * ROWS }, (_, i) => {
    const c = i % COLS, r = Math.floor(i / COLS);
    const x = (c - (COLS - 1) / 2) * SPACING;
    const z = (r - (ROWS - 1) / 2) * SPACING;
    // A gentle undulation keeps the field three-dimensional without randomness.
    const y = 0.09 * Math.sin(0.9 * c) * Math.cos(0.7 * r);
    return [+x.toFixed(4), +y.toFixed(4), +z.toFixed(4)] as const;
  });
}

function gridEdges(): [number, number][] {
  const edges: [number, number][] = [];
  for (let i = 0; i < COLS * ROWS; i++) {
    const c = i % COLS, r = Math.floor(i / COLS);
    if (c + 1 < COLS) edges.push([i, i + 1]);
    if (r + 1 < ROWS) edges.push([i, i + COLS]);
  }
  return edges;
}

export function buildModel(): Model {
  const points = lattice();
  const edges = gridEdges();
  const core = Math.floor(ROWS / 2) * COLS + Math.floor(COLS / 2);
  const dist = points.map((p) => Math.hypot(p[0] - points[core][0], p[2] - points[core][2]));
  const maxDist = Math.max(...dist);
  const ignite = dist.map((d) => (d === 0 ? 0 : frontTimeFor(d, maxDist)));
  const validators = points
    .map((_, i) => i)
    .filter((i) => i !== core && (i % COLS) % VALIDATOR_EVERY_COL === 1 && Math.floor(i / COLS) % VALIDATOR_EVERY_ROW === 0);
  const quorum = Math.ceil((validators.length * 2) / 3);
  const mlStart = validators.map((v) => ignite[v] + 0.4);
  const slhStart = mlStart.map((s) => s + SLH_GAP);
  const slhArrive = slhStart.map((s) => s + BEAM_TRAVEL).sort((a, b) => a - b);
  return {
    points, edges, core, dist, maxDist, validators, quorum, ignite, mlStart, slhStart,
    travel: BEAM_TRAVEL,
    signStart: Math.min(...mlStart),
    finalAt: slhArrive[quorum - 1],
  };
}

const fade = (t: number) => 1 - ease((t - (PERIOD - 1.2)) / 1.2);

export function phaseAt(m: Model, t: number): number {
  if (t < FRONT_START) return 0;
  if (t < m.signStart) return 1;
  if (t < m.finalAt) return 2;
  return 3;
}

// Nodes light only when the front reaches them: an eased rise, a contact flash,
// then a slow decay that leaves a dim trace.
export function nodeLevel(m: Model, i: number, t: number): number {
  const s = t - m.ignite[i];
  if (s < 0) return 0;
  const rise = easeOut(s / RISE);
  const decay = Math.exp(-Math.max(0, s - RISE) / 1.8);
  return (rise * (0.22 + 0.78 * decay)) * fade(t);
}

// Impact flash on contact, in [0, 1]; a sharp spike that dies in a fraction of a second.
export function contactFlash(m: Model, i: number, t: number): number {
  const s = t - m.ignite[i];
  return s < 0 ? 0 : Math.exp(-s * 7);
}

// How far the lattice is lifted at t, in field units: a shell that follows the front.
// Nodes just ahead of the crest feel a smaller pre-lift than those under it.
export function liftAt(m: Model, i: number, t: number): number {
  const R = frontRadius(m, t);
  if (R === 0) return 0;
  const d = m.dist[i];
  const w = d > R ? SHELL_W * 2.2 : SHELL_W;
  const x = (d - R) / w;
  return SHELL_AMP * Math.exp(-x * x) * (d > R ? 0.2 : 1);
}

// The lock: edges brighten, the lattice draws in toward the core a few percent.
export function lockAt(m: Model, t: number): number {
  return ease((t - m.finalAt) / 0.5) * fade(t);
}

export function edgeLevel(m: Model, e: number, t: number): number {
  const [a, b] = m.edges[e];
  const s = t - Math.min(m.ignite[a], m.ignite[b]);
  if (s < 0) return 0;
  const flash = Math.exp(-s * 6);
  const lit = easeOut(s / 0.4) * 0.3;
  return Math.max(lit, flash) * fade(t) * (1 + 0.8 * lockAt(m, t));
}

// Signature beam k (validator index), kind 0 = ML-DSA, 1 = SLH-DSA.
export function beamAt(m: Model, k: number, kind: 0 | 1, t: number): { p: number; alpha: number } {
  const start = kind === 0 ? m.mlStart[k] : m.slhStart[k];
  const arrive = start + m.travel;
  if (t < start) return { p: 0, alpha: 0 };
  return {
    p: ease((t - start) / m.travel),
    alpha: t < arrive ? 1 : clamp01(1 - (t - arrive) / 0.4) * fade(t),
  };
}

// Finality: a bloom swell that decays, a slow pulse that holds, and a short shake.
export function finalityAt(m: Model, t: number): { flash: number; pulse: number; shake: number } {
  const s = t - m.finalAt;
  if (s < 0) return { flash: 0, pulse: 0, shake: 0 };
  return {
    flash: Math.exp(-s * 1.6),
    pulse: 0.8 + 0.2 * Math.sin((2 * Math.PI * s) / 3.2),
    shake: s < 1.5 ? CLOCK_SHAKE * Math.exp(-3.2 * s) : 0,
  };
}

// Camera push-in: 0 at rest, 1 at the impact, held through the lock, eased back out.
export function pushAt(m: Model, t: number): number {
  if (t < FRONT_START) return 0;
  if (t < m.finalAt) return ease((t - FRONT_START) / (m.finalAt - FRONT_START));
  if (t < m.finalAt + HOLD) return 1;
  return 1 - ease((t - m.finalAt - HOLD) / (PERIOD - m.finalAt - HOLD));
}

export function cameraDistance(m: Model, t: number): number {
  return WIDE - PUSH_DEPTH * pushAt(m, t);
}

// Camera position: elevated, looking down onto the field.
export function cameraPosition(m: Model, t: number): Vec3 {
  const d = cameraDistance(m, t);
  return [0, d * 0.5, d * 0.87];
}

// Heavy, slow yaw of the field: one swing per cycle.
export const yawAt = (t: number) => 0.12 * Math.sin((2 * Math.PI * t) / PERIOD);

// The wavefront ring: radius and alpha. It is faded in at birth and out as the
// crest reaches the far edge of the field.
export function ringAt(m: Model, t: number): { r: number; a: number } {
  const r = frontRadius(m, t);
  const inFront = t - FRONT_START;
  const a = inFront < 0 ? 0 : Math.min(1, inFront / 0.2) * (1 - ease((inFront - FRONT_DUR) / 0.6));
  return { r, a: clamp01(a) };
}

// Project a world point to the screen of a camera at cam looking at the origin.
// Returns coordinates in the same units as the field (x right, y up).
export function project(p: Vec3, cam: Vec3, focal = 9): [number, number] {
  const f = normalize([-cam[0], -cam[1], -cam[2]]);
  const r = normalize(cross(f, [0, 1, 0]));
  const u = cross(r, f);
  const v = [p[0] - cam[0], p[1] - cam[1], p[2] - cam[2]] as const;
  const z = dot(v, f);
  return [(dot(v, r) * focal) / z, (dot(v, u) * focal) / z];
}

const dot = (a: readonly number[], b: readonly number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: readonly number[], b: readonly number[]): Vec3 => [
  a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0],
];
const normalize = (a: readonly number[]): Vec3 => {
  const n = Math.hypot(a[0], a[1], a[2]) || 1;
  return [a[0] / n, a[1] / n, a[2] / n];
};

export { FRONT_START, FRONT_DUR, CLOCK_SHAKE };
