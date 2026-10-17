// Plain math for the 2-D lattice demo. No DOM here, so the tests can run it
// under `node --test` without a browser.

export type Vec = readonly [number, number];
export type Basis = readonly [Vec, Vec];

/** The point i·b1 + j·b2 of the lattice the basis generates. */
export function latticePoint(basis: Basis, i: number, j: number): Vec {
  const [b1, b2] = basis;
  return [i * b1[0] + j * b2[0], i * b1[1] + j * b2[1]];
}

export function norm(v: Vec): number {
  return Math.hypot(v[0], v[1]);
}

/**
 * The lattice point nearest `target`, searched over coefficients in
 * [-range, range]. Brute force on purpose: it is the naive search a person
 * would do by eye, and its cost is what the demo is about.
 */
export function nearestLatticePoint(
  basis: Basis,
  target: Vec,
  range: number,
): { i: number; j: number; point: Vec; tries: number } {
  let best = { i: 0, j: 0, point: [0, 0] as Vec, dist: Infinity };
  let tries = 0;
  for (let i = -range; i <= range; i++) {
    for (let j = -range; j <= range; j++) {
      tries++;
      const point = latticePoint(basis, i, j);
      const dist = Math.hypot(point[0] - target[0], point[1] - target[1]);
      if (dist < best.dist) best = { i, j, point, dist };
    }
  }
  return { i: best.i, j: best.j, point: best.point, tries };
}

/**
 * Gauss (Lagrange) reduction of a 2-D basis: repeatedly subtract the nearest
 * integer multiple of the shorter vector from the longer one. Its output
 * spans the same lattice with the two shortest independent vectors, which is
 * why a short basis reveals short vectors and a long one hides them.
 */
export function gaussReduce(basis: Basis): Basis {
  let a: Vec = basis[0];
  let b: Vec = basis[1];
  for (;;) {
    if (norm(b) < norm(a)) [a, b] = [b, a];
    const dot = a[0] * b[0] + a[1] * b[1];
    const aa = a[0] * a[0] + a[1] * a[1];
    const m = Math.round(dot / aa);
    if (m === 0) return [a, b];
    b = [b[0] - m * a[0], b[1] - m * a[1]];
  }
}

/** Determinant of the basis: the area of one fundamental cell. */
export function determinant(basis: Basis): number {
  return basis[0][0] * basis[1][1] - basis[0][1] * basis[1][0];
}
