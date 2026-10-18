// Pure model of the home-page propagation scene. Everything on screen is a
// function of simulated time t, so the loop cannot drift and the static poster
// is the same model drawn once. No randomness: the graph is a Fibonacci sphere
// and its edges are the nearest neighbours, so every visit shows the same block.
//
// The scene is an illustration of the protocol, not a measurement. No number on
// it is a chain parameter: the validator stride and the 2/3 quorum are picked
// for legibility. The page's caption says so.

export const PERIOD = 12; // seconds per block cycle
export const NODE_COUNT = 120;
export const RADIUS = 1.6;
const NEAREST = 3;
const VALIDATOR_STRIDE = 6;
const BIRTH_END = 0.9; // the core is born in [0, BIRTH_END)
const HOP_DELAY = 0.42; // seconds between graph hops as the front passes
const RISE = 0.55; // ignition time of one node, with weight
const BEAM_TRAVEL = 1.1; // a signature beam flying back to the core
const SLH_GAP = 0.35; // the second signature starts after the first
const FADE = 1.2; // every light fades before the loop wraps

export const PHASES = ["proposed", "propagated", "signed twice (ML-DSA + SLH-DSA)", "final"] as const;

export type Vec3 = readonly [number, number, number];

export type Model = {
  points: Vec3[];
  edges: [number, number][];
  core: number;
  validators: number[];
  quorum: number;
  ignite: number[]; // per node: time its ignition starts
  mlStart: number[]; // per validator (same order as `validators`)
  slhStart: number[];
  travel: number;
  signStart: number;
  finalAt: number;
};

const dist2 = (a: Vec3, b: Vec3) => (a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 + (a[2] - b[2]) ** 2;

function fibonacci(n: number, r: number): Vec3[] {
  const golden = Math.PI * (3 - Math.sqrt(5));
  return Array.from({ length: n }, (_, i) => {
    const y = 1 - (2 * (i + 0.5)) / n;
    const rad = Math.sqrt(1 - y * y);
    const th = golden * i;
    return [+(Math.cos(th) * rad * r).toFixed(4), +(y * r).toFixed(4), +(Math.sin(th) * rad * r).toFixed(4)] as const;
  });
}

function nearestEdges(points: Vec3[], k: number): [number, number][] {
  const seen = new Set<string>();
  const edges: [number, number][] = [];
  points.forEach((p, i) => {
    points
      .map((q, j) => ({ j, d: j === i ? Infinity : dist2(p, q) }))
      .sort((a, b) => a.d - b.d || a.j - b.j)
      .slice(0, k)
      .forEach(({ j }) => {
        const key = i < j ? `${i}-${j}` : `${j}-${i}`;
        if (!seen.has(key)) { seen.add(key); edges.push([Math.min(i, j), Math.max(i, j)]); }
      });
  });
  return edges;
}

function bfsHops(n: number, edges: [number, number][], from: number): number[] {
  const adj: number[][] = Array.from({ length: n }, () => []);
  for (const [a, b] of edges) { adj[a].push(b); adj[b].push(a); }
  const hops = new Array<number>(n).fill(-1);
  hops[from] = 0;
  const queue = [from];
  for (let q = 0; q < queue.length; q++) {
    const u = queue[q];
    for (const v of adj[u]) if (hops[v] < 0) { hops[v] = hops[u] + 1; queue.push(v); }
  }
  return hops;
}

export function buildModel(): Model {
  const points = fibonacci(NODE_COUNT, RADIUS);
  const edges = nearestEdges(points, NEAREST);
  const front: Vec3 = [0, 0, RADIUS];
  const core = points.reduce((best, p, i) => (dist2(p, front) < dist2(points[best], front) ? i : best), 0);
  const hops = bfsHops(NODE_COUNT, edges, core);
  // The core ignites from t=0 (birth); its neighbours are the first hop after birth.
  const ignite = hops.map((h) => (h === 0 ? 0 : BIRTH_END + (h - 1) * HOP_DELAY));
  const validators = points.map((_, i) => i).filter((i) => i % VALIDATOR_STRIDE === 0 && i !== core);
  const quorum = Math.ceil((validators.length * 2) / 3);
  const mlStart = validators.map((v) => ignite[v] + RISE * 0.8);
  const slhStart = mlStart.map((s) => s + SLH_GAP);
  const slhArrive = slhStart.map((s) => s + BEAM_TRAVEL).sort((a, b) => a - b);
  return {
    points, edges, core, validators, quorum, ignite, mlStart, slhStart,
    travel: BEAM_TRAVEL,
    signStart: Math.min(...mlStart),
    finalAt: slhArrive[quorum - 1],
  };
}

const clamp01 = (x: number) => Math.min(1, Math.max(0, x));
// Ease-in-out: slow start, slow finish. Heavy, never linear.
export const ease = (x: number) => { const y = clamp01(x); return y < 0.5 ? 4 * y * y * y : 1 - (-2 * y + 2) ** 3 / 2; };
const fade = (t: number) => 1 - ease((t - (PERIOD - FADE)) / FADE);

export function phaseAt(m: Model, t: number): number {
  if (t < BIRTH_END) return 0;
  if (t < m.signStart) return 1;
  if (t < m.finalAt) return 2;
  return 3;
}

// Light of node i in [0, 1]: ignition ramps in with weight, then holds.
export function nodeLevel(m: Model, i: number, t: number): number {
  return ease((t - m.ignite[i]) / RISE) * fade(t);
}

// Edges flash as the front crosses them, then settle to a dim trace.
export function edgeLevel(m: Model, e: number, t: number): number {
  const [a, b] = m.edges[e];
  const from = Math.min(m.ignite[a], m.ignite[b]);
  const s = t - from;
  if (s < 0) return 0;
  const flash = ease(s / 0.25) * (1 - 0.65 * ease((s - 0.25) / 0.9));
  return Math.max(0.35 * ease(s / 0.4), flash) * fade(t);
}

// Signature beam k (validator index), kind 0 = ML-DSA, 1 = SLH-DSA.
// Returns the fraction of the flight covered, and whether the beam is still visible.
export function beamAt(m: Model, k: number, kind: 0 | 1, t: number): { p: number; alpha: number } {
  const start = kind === 0 ? m.mlStart[k] : m.slhStart[k];
  const arrive = start + m.travel;
  if (t < start) return { p: 0, alpha: 0 };
  const p = ease((t - start) / m.travel);
  return { p, alpha: t < arrive ? 1 : clamp01(1 - (t - arrive) / 0.4) * fade(t) };
}

// Quorum lock: a flash that decays, a slow pulse, and a short camera shake.
export function finalityAt(m: Model, t: number): { flash: number; pulse: number; shake: number } {
  const s = t - m.finalAt;
  if (s < 0) return { flash: 0, pulse: 0, shake: 0 };
  return {
    flash: Math.exp(-s * 1.6),
    pulse: 0.82 + 0.18 * Math.sin((2 * Math.PI * s) / 3.2),
    shake: s < 1.5 ? 0.035 * Math.exp(-3.2 * s) : 0,
  };
}

// Heavy, slow yaw of the camera: one swing per cycle.
export const yawAt = (t: number) => 0.18 * Math.sin((2 * Math.PI * t) / PERIOD);

// Pressure ring radius, in scene units, over the propagation window.
export function ringAt(m: Model, t: number): { r: number; a: number } {
  const end = Math.max(...m.ignite) + RISE;
  const p = clamp01((t - BIRTH_END) / (end - BIRTH_END));
  // Stays inside the canvas at every aspect ratio the hero uses (the camera sees about +-2.8 units).
  return { r: 0.2 + 1.8 * ease(p), a: t < BIRTH_END ? 0 : (1 - p) * 0.8 };
}
