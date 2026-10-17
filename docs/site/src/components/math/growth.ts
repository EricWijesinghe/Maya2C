// log2 of the work needed to break each primitive, as a function of a size
// parameter. Formulas, not measurements:
//   brute force on an s-bit secret ............ 2^s operations
//   Grover on an s-bit secret ................. (pi/4) * 2^(s/2) iterations (Grover, 1996)
//   BKZ-beta sieving, classical ............... 2^(0.292 beta) (docs/lattice-pow.md)
//   BKZ-beta sieving, quantum ................. 2^(0.265 beta) (Laarhoven et al. 2015, literature)
//   Shor on an s-bit modulus .................. polynomial: about s^3 gate operations (Shor, 1994/1997)
// Exponential in the parameter for the first four; polynomial for Shor.

export const SERIES = ["brute", "grover", "sieve", "sieveQuantum", "shor"] as const;
export type Series = (typeof SERIES)[number];

export function log2Cost(series: Series, s: number): number {
  switch (series) {
    case "brute":
      return s;
    case "grover":
      return s / 2 + Math.log2(Math.PI / 4);
    case "sieve":
      return 0.292 * s;
    case "sieveQuantum":
      return 0.265 * s;
    case "shor":
      return 3 * Math.log2(s);
  }
}
