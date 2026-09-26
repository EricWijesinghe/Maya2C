// BLAKE3, written from the BLAKE3 specification's reference implementation
// (github.com/BLAKE3-team/BLAKE3, reference_impl/reference_impl.rs) for the
// independent verifier. No dependency: a second opinion that imported the
// same hash library as the node would share its bugs. Hash mode only, 32-byte
// output. Checked against the official test vectors by `selftest()`.

const IV = new Uint32Array([
  0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
]);
const MSG_PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;
const DERIVE_KEY_CONTEXT = 32;
const DERIVE_KEY_MATERIAL = 64;
const BLOCK_LEN = 64;
const CHUNK_LEN = 1024;

function rotr(x: number, n: number): number {
  return ((x >>> n) | (x << (32 - n))) >>> 0;
}

function g(s: Uint32Array, a: number, b: number, c: number, d: number, mx: number, my: number): void {
  s[a] = (s[a] + s[b] + mx) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 16);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 12);
  s[a] = (s[a] + s[b] + my) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 8);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 7);
}

function round(s: Uint32Array, m: Uint32Array): void {
  g(s, 0, 4, 8, 12, m[0], m[1]);
  g(s, 1, 5, 9, 13, m[2], m[3]);
  g(s, 2, 6, 10, 14, m[4], m[5]);
  g(s, 3, 7, 11, 15, m[6], m[7]);
  g(s, 0, 5, 10, 15, m[8], m[9]);
  g(s, 1, 6, 11, 12, m[10], m[11]);
  g(s, 2, 7, 8, 13, m[12], m[13]);
  g(s, 3, 4, 9, 14, m[14], m[15]);
}

function compress(cv: Uint32Array, block: Uint32Array, counter: number, blockLen: number, flags: number): Uint32Array {
  const s = new Uint32Array(16);
  s.set(cv, 0);
  s.set(IV.subarray(0, 4), 8);
  s[12] = counter >>> 0;
  s[13] = Math.floor(counter / 0x100000000) >>> 0;
  s[14] = blockLen;
  s[15] = flags;
  let m = Uint32Array.from(block);
  for (let r = 0; r < 7; r++) {
    round(s, m);
    if (r < 6) m = Uint32Array.from(MSG_PERMUTATION, (i) => m[i]);
  }
  for (let i = 0; i < 8; i++) {
    s[i] ^= s[i + 8];
    s[i + 8] ^= cv[i];
  }
  return s;
}

function words(bytes: Uint8Array): Uint32Array {
  const padded = new Uint8Array(BLOCK_LEN);
  padded.set(bytes);
  const w = new Uint32Array(16);
  for (let i = 0; i < 16; i++) {
    w[i] = (padded[4 * i] | (padded[4 * i + 1] << 8) | (padded[4 * i + 2] << 16) | (padded[4 * i + 3] << 24)) >>> 0;
  }
  return w;
}

/** An output node, finalized with ROOT only if it turns out to be the root. */
interface Output {
  cv: Uint32Array;
  block: Uint32Array;
  blockLen: number;
  counter: number;
  flags: number;
}

function chunkOutput(chunk: Uint8Array, counter: number, key: Uint32Array, mode: number): Output {
  let cv = key.slice();
  const blocks = Math.max(1, Math.ceil(chunk.length / BLOCK_LEN));
  for (let b = 0; b < blocks - 1; b++) {
    const flags = (b === 0 ? CHUNK_START : 0) | mode;
    cv = compress(cv, words(chunk.subarray(b * BLOCK_LEN, (b + 1) * BLOCK_LEN)), counter, BLOCK_LEN, flags).slice(0, 8);
  }
  const last = chunk.subarray((blocks - 1) * BLOCK_LEN);
  const flags = (blocks === 1 ? CHUNK_START : 0) | CHUNK_END | mode;
  return { cv, block: words(last), blockLen: last.length, counter, flags };
}

function chainingValue(o: Output): Uint32Array {
  return compress(o.cv, o.block, o.counter, o.blockLen, o.flags).slice(0, 8);
}

function parentOutput(left: Uint32Array, right: Uint32Array, key: Uint32Array, mode: number): Output {
  const block = new Uint32Array(16);
  block.set(left, 0);
  block.set(right, 8);
  return { cv: key.slice(), block, blockLen: BLOCK_LEN, counter: 0, flags: PARENT | mode };
}

function hashWith(input: Uint8Array, key: Uint32Array, mode: number): Uint8Array {
  const chunks = Math.max(1, Math.ceil(input.length / CHUNK_LEN));
  const stack: Uint32Array[] = [];
  let out: Output | undefined;
  for (let c = 0; c < chunks; c++) {
    out = chunkOutput(input.subarray(c * CHUNK_LEN, (c + 1) * CHUNK_LEN), c, key, mode);
    if (c === chunks - 1) break;
    // Merge completed subtrees: one merge per trailing zero bit of the new total.
    let cv = chainingValue(out);
    let total = c + 1;
    while ((total & 1) === 0) {
      cv = chainingValue(parentOutput(stack.pop() as Uint32Array, cv, key, mode));
      total >>= 1;
    }
    stack.push(cv);
  }
  let node = out as Output;
  while (stack.length > 0) {
    node = parentOutput(stack.pop() as Uint32Array, chainingValue(node), key, mode);
  }
  const s = compress(node.cv, node.block, node.counter, node.blockLen, node.flags | ROOT);
  const bytes = new Uint8Array(32);
  for (let i = 0; i < 8; i++) {
    bytes[4 * i] = s[i] & 0xff;
    bytes[4 * i + 1] = (s[i] >>> 8) & 0xff;
    bytes[4 * i + 2] = (s[i] >>> 16) & 0xff;
    bytes[4 * i + 3] = (s[i] >>> 24) & 0xff;
  }
  return bytes;
}

/** BLAKE3 hash of `input`, 32 bytes. */
export function blake3(input: Uint8Array): Uint8Array {
  return hashWith(input, IV, 0);
}

/** BLAKE3 derive-key mode: the context string keys a hash of the material. */
export function deriveKey(context: string, material: Uint8Array): Uint8Array {
  const ctx = hashWith(new TextEncoder().encode(context), IV, DERIVE_KEY_CONTEXT);
  const key = new Uint32Array(8);
  for (let i = 0; i < 8; i++) key[i] = (ctx[4 * i] | (ctx[4 * i + 1] << 8) | (ctx[4 * i + 2] << 16) | (ctx[4 * i + 3] << 24)) >>> 0;
  return hashWith(material, key, DERIVE_KEY_MATERIAL);
}

/** Official BLAKE3 test vectors: input byte i is i % 251; first 32 output bytes. */
const VECTORS: Array<[number, string]> = [
  [0, "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"],
  [1, "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213"],
  [1023, "10108970eeda3eb932baac1428c7a2163b0e924c9a9e25b35bba72b28f70bd11"],
  [1024, "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7"],
  [1025, "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444"],
  [2048, "e776b6028c7cd22a4d0ba182a8bf62205d2ef576467e838ed6f2529b85fba24a"],
  [3072, "b98cb0ff3623be03326b373de6b9095218513e64f1ee2edd2525c7ad1e5cffd2"],
  [8192, "aae792484c8efe4f19e2ca7d371d8c467ffb10748d8a5a1ae579948f718a2a63"],
];

/** Official derive-key vectors (same inputs), computed with the reference context. */
const DERIVE_CONTEXT = "BLAKE3 2019-12-27 16:29:52 test vectors context";
const DERIVE_VECTORS: Array<[number, string]> = [
  [0, "2cc39783c223154fea8dfb7c1b1660f2ac2dcbd1c1de8277b0b0dd39b7e50d7d"],
  [1, "b3e2e340a117a499c6cf2398a19ee0d29cca2bb7404c73063382693bf66cb06c"],
  [1024, "7356cd7720d5b66b6d0697eb3177d9f8d73a4a5c5e968896eb6a689684302706"],
  [1025, "effaa245f065fbf82ac186839a249707c3bddf6d3fdda22d1b95a3c970379bcb"],
  [3072, "050df97f8c2ead654d9bb3ab8c9178edcd902a32f8495949feadcc1e0480c46b"],
];

export function selftest(): void {
  for (const [len, want] of VECTORS) {
    const input = Uint8Array.from({ length: len }, (_, i) => i % 251);
    const got = toHex(blake3(input));
    if (got !== want) throw new Error(`BLAKE3 self-test failed at length ${len}: ${got}`);
  }
  for (const [len, want] of DERIVE_VECTORS) {
    const input = Uint8Array.from({ length: len }, (_, i) => i % 251);
    const got = toHex(deriveKey(DERIVE_CONTEXT, input));
    if (got !== want) throw new Error(`BLAKE3 derive-key self-test failed at length ${len}: ${got}`);
  }
}

export function toHex(b: Uint8Array): string {
  return Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");
}

export function fromHex(s: string): Uint8Array {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.slice(2 * i, 2 * i + 2), 16);
  return out;
}
