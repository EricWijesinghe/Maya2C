// A binary hash tree with SHA-256 from the platform (Web Crypto, same origin,
// no library). Leaves and inner nodes are domain-separated so a leaf can never
// be read as an inner node. Power-of-two leaf counts only.

export type Side = "left" | "right";
export interface Step {
  /** Hash of the sibling node needed to climb one level. */
  sibling: string;
  /** Which side the sibling sits on. */
  side: Side;
}

export async function sha256Hex(text: string): Promise<string> {
  const bytes = new TextEncoder().encode(text);
  const digest = await globalThis.crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

const leafHash = (value: string) => sha256Hex(`leaf|${value}`);
const nodeHash = (left: string, right: string) => sha256Hex(`node|${left}|${right}`);

/** levels[0] are the leaf hashes; the last level is the root. */
export async function buildLevels(values: readonly string[]): Promise<string[][]> {
  if (values.length === 0 || (values.length & (values.length - 1)) !== 0) {
    throw new RangeError("leaf count must be a non-zero power of two");
  }
  const levels: string[][] = [await Promise.all(values.map(leafHash))];
  while (levels[levels.length - 1].length > 1) {
    const below = levels[levels.length - 1];
    const above: string[] = [];
    for (let i = 0; i < below.length; i += 2) above.push(await nodeHash(below[i], below[i + 1]));
    levels.push(above);
  }
  return levels;
}

/** The sibling hashes that prove leaf `index` is under the root. */
export function proofFor(levels: string[][], index: number): Step[] {
  const steps: Step[] = [];
  let at = index;
  for (let level = 0; level < levels.length - 1; level++) {
    const isRight = at % 2 === 1;
    steps.push({
      sibling: levels[level][isRight ? at - 1 : at + 1],
      side: isRight ? "left" : "right",
    });
    at = Math.floor(at / 2);
  }
  return steps;
}

/** Recomputes the root from one leaf value and its proof. */
export async function rootFromProof(value: string, steps: readonly Step[]): Promise<string> {
  let hash = await leafHash(value);
  for (const step of steps) {
    hash = step.side === "left"
      ? await nodeHash(step.sibling, hash)
      : await nodeHash(hash, step.sibling);
  }
  return hash;
}
