/**
 * Passkey unlock for a locally stored post-quantum seed (parity job P-17).
 *
 * # Threat model
 *
 * The chain only ever sees ML-DSA-65 + SLH-DSA signatures. The passkey is a
 * classical P-256 credential and **never signs a transaction**. It only
 * unlocks the seed: the WebAuthn PRF extension (CTAP2 `hmac-secret`) returns a
 * secret the authenticator computes from a key that never leaves the device,
 * HKDF turns it into an AES-256-GCM key-encryption key, and that key unwraps
 * the seed in memory. An attacker who breaks P-256 learns nothing that helps:
 * the P-256 public key is never published, and even with the private key they
 * still need the authenticator's separate PRF secret. That secret lives where
 * the passkey lives: on one device for a device-bound key (a security key,
 * some platform authenticators), but for a *synced* passkey (iCloud Keychain,
 * Google Password Manager, Windows) on every device signed in to that account
 * and in the provider's sync store. WebAuthn has no flag a page can use to
 * forbid sync. So stealing funds needs an authenticator holding the passkey
 * (or the fallback password), plus whatever user verification it demands.
 *
 * What this does not protect against: malware running in the page while the
 * seed is unlocked, or a weak fallback password (PBKDF2 slows guessing, it
 * does not stop it).
 *
 * # Zeroing
 *
 * Every intermediate buffer this module owns is filled with zeros before it
 * returns. JavaScript cannot promise more than that: the engine may have
 * copied a buffer, a `password` string is immutable and cannot be zeroed, and
 * the seed handed back is the caller's to zero. User verification is requested
 * as "required" and enforced by the browser; this module does not re-check the
 * UV flag (a different UV state yields a different PRF output, so unlock fails).
 */

/** Why an unlock or wrap was refused. Messages never contain secret bytes. */
export type PasskeyErrorKind =
  | "prf-unsupported" // the authenticator or browser returned no PRF output
  | "unlock-failed" // wrong passkey, wrong password, or a tampered envelope
  | "bad-envelope" // the stored envelope is not one this version reads
  | "no-method"; // neither a passkey nor a password was offered

export class PasskeyError extends Error {
  constructor(
    readonly kind: PasskeyErrorKind,
    message: string,
  ) {
    super(message);
    this.name = "PasskeyError";
  }
}

/** The subset of `navigator.credentials` this module calls; tests pass a mock. */
export interface CredentialsLike {
  create(options: unknown): Promise<unknown>;
  get(options: unknown): Promise<unknown>;
}

/** Which passkey and relying party an envelope is bound to. */
export interface PasskeyRef {
  /** base64url credential id, as `registerPasskey` returned it. */
  credentialId: string;
  rpId: string;
}

/** An encrypted seed. Every field is public; the seed is only in `ciphertext`. */
export type SeedEnvelope =
  | {
      v: 1;
      kind: "passkey-prf";
      credentialId: string;
      rpId: string;
      prfSalt: string;
      hkdfSalt: string;
      iv: string;
      ciphertext: string;
    }
  | {
      v: 1;
      kind: "password";
      iterations: number;
      salt: string;
      iv: string;
      ciphertext: string;
    };

// Omit over each union member, so the object literals below keep their discriminant checks.
type EnvelopeHeader = SeedEnvelope extends infer E ? (E extends SeedEnvelope ? Omit<E, "ciphertext"> : never) : never;

const SALT_BYTES = 32;
const IV_BYTES = 12;
const CHALLENGE_BYTES = 32;
const ES256 = -7;
const RS256 = -257;
const HKDF_INFO = "maya2c/passkey-kek/v1";
/** OWASP's 2023 figure for PBKDF2-HMAC-SHA256. */
export const PBKDF2_ITERATIONS = 600_000;
// shortcut: PBKDF2 because WebCrypto has no memory-hard KDF; move to Argon2id when the wasm SDK exposes one.

const subtle = (): SubtleCrypto => globalThis.crypto.subtle;
const random = (n: number): Uint8Array<ArrayBuffer> => globalThis.crypto.getRandomValues(new Uint8Array(n));
const utf8 = (s: string): Uint8Array<ArrayBuffer> => new TextEncoder().encode(s);

export function toB64u(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromB64u(text: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) throw new PasskeyError("bad-envelope", "not base64url");
  const bin = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

/**
 * Every envelope field but the ciphertext is authenticated as AES-GCM
 * additional data. JSON array encoding, not a separator join: fields such as
 * `rpId` are free text, and an array keeps field boundaries unambiguous.
 */
function aad(e: SeedEnvelope): Uint8Array<ArrayBuffer> {
  const head =
    e.kind === "passkey-prf"
      ? [e.v, e.kind, e.credentialId, e.rpId, e.prfSalt, e.hkdfSalt, e.iv]
      : [e.v, e.kind, e.iterations, e.salt, e.iv];
  return utf8(JSON.stringify(head));
}

/** Refuses PBKDF2 below the module's floor, on wrap and on unlock. */
function checkIterations(iterations: number): void {
  if (!Number.isInteger(iterations) || iterations < PBKDF2_ITERATIONS) {
    throw new PasskeyError("bad-envelope", `PBKDF2 iterations below ${PBKDF2_ITERATIONS}`);
  }
}

async function seal(key: CryptoKey, seed: Uint8Array, header: EnvelopeHeader): Promise<SeedEnvelope> {
  const env = { ...header, ciphertext: "" } as SeedEnvelope;
  const plain = new Uint8Array(seed);
  try {
    const ct = await subtle().encrypt(
      { name: "AES-GCM", iv: fromB64u(env.iv), additionalData: aad(env) },
      key,
      plain,
    );
    return { ...env, ciphertext: toB64u(new Uint8Array(ct)) };
  } finally {
    plain.fill(0);
  }
}

async function open(key: CryptoKey, env: SeedEnvelope): Promise<Uint8Array> {
  try {
    const pt = await subtle().decrypt(
      { name: "AES-GCM", iv: fromB64u(env.iv), additionalData: aad(env) },
      key,
      fromB64u(env.ciphertext),
    );
    return new Uint8Array(pt);
  } catch {
    throw new PasskeyError("unlock-failed", "the seed did not decrypt: wrong credential, wrong password, or a modified envelope");
  }
}

/**
 * HKDF-SHA256 over the PRF output; the KEK is non-extractable. Zeroes `prf`.
 * `info` binds the relying party and credential (defence in depth; the random
 * PRF salt already separates envelopes).
 */
async function kekFromPrf(prf: Uint8Array<ArrayBuffer>, hkdfSalt: Uint8Array<ArrayBuffer>, ref: PasskeyRef): Promise<CryptoKey> {
  try {
    const ikm = await subtle().importKey("raw", prf, "HKDF", false, ["deriveKey"]);
    return await subtle().deriveKey(
      { name: "HKDF", hash: "SHA-256", salt: hkdfSalt, info: utf8(JSON.stringify([HKDF_INFO, ref.rpId, ref.credentialId])) },
      ikm,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt", "decrypt"],
    );
  } finally {
    prf.fill(0);
  }
}

async function kekFromPassword(password: string, salt: Uint8Array<ArrayBuffer>, iterations: number): Promise<CryptoKey> {
  const pw = utf8(password);
  try {
    const base = await subtle().importKey("raw", pw, "PBKDF2", false, ["deriveKey"]);
    return await subtle().deriveKey(
      { name: "PBKDF2", hash: "SHA-256", salt, iterations },
      base,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt", "decrypt"],
    );
  } finally {
    pw.fill(0);
  }
}

interface PrfExtensionResults {
  prf?: { enabled?: boolean; results?: { first?: ArrayBuffer | ArrayBufferView } };
}

function extensionResults(cred: unknown): PrfExtensionResults {
  const c = cred as { getClientExtensionResults?: () => PrfExtensionResults } | null;
  return c?.getClientExtensionResults?.() ?? {};
}

function credentials(given?: CredentialsLike): CredentialsLike {
  const c = given ?? (globalThis.navigator?.credentials as CredentialsLike | undefined);
  if (!c) throw new PasskeyError("prf-unsupported", "this environment has no WebAuthn credentials API");
  return c;
}

/** Asks the authenticator for PRF(prfSalt). Returns null when PRF is not supported. */
async function evaluatePrf(ref: PasskeyRef, prfSalt: Uint8Array<ArrayBuffer>, given?: CredentialsLike): Promise<Uint8Array<ArrayBuffer> | null> {
  const cred = await credentials(given).get({
    publicKey: {
      challenge: random(CHALLENGE_BYTES),
      rpId: ref.rpId,
      allowCredentials: [{ type: "public-key", id: fromB64u(ref.credentialId) }],
      userVerification: "required",
      extensions: { prf: { eval: { first: prfSalt } } },
    },
  });
  const first = extensionResults(cred).prf?.results?.first;
  if (!first) return null;
  const view = ArrayBuffer.isView(first) ? new Uint8Array(first.buffer, first.byteOffset, first.byteLength) : new Uint8Array(first);
  const out = new Uint8Array(view);
  view.fill(0);
  return out;
}

export interface RegisterOptions {
  rp: { id: string; name: string };
  user: { id: Uint8Array; name: string; displayName: string };
  credentials?: CredentialsLike;
}

/**
 * Creates a discoverable passkey with the PRF extension requested. The
 * challenge is random and checked by nobody: no server is involved, and the
 * credential's public key is never used.
 */
export async function registerPasskey(opts: RegisterOptions): Promise<{ ref: PasskeyRef; prfEnabled: boolean }> {
  const cred = (await credentials(opts.credentials).create({
    publicKey: {
      challenge: random(CHALLENGE_BYTES),
      rp: opts.rp,
      user: { ...opts.user, id: new Uint8Array(opts.user.id) },
      pubKeyCredParams: [
        { type: "public-key", alg: ES256 },
        { type: "public-key", alg: RS256 },
      ],
      authenticatorSelection: { residentKey: "required", userVerification: "required" },
      extensions: { prf: {} },
    },
  })) as { rawId?: ArrayBuffer } | null;
  if (!cred?.rawId) throw new PasskeyError("prf-unsupported", "no credential was created");
  // Enforced, not reported: a passkey without PRF cannot wrap a seed, and a
  // caller that ignored a `false` would only find out later.
  if (extensionResults(cred).prf?.enabled !== true) {
    throw new PasskeyError("prf-unsupported", "the authenticator created a passkey without PRF support");
  }
  return { ref: { credentialId: toB64u(new Uint8Array(cred.rawId)), rpId: opts.rp.id }, prfEnabled: true };
}

/** Wraps `seed` under a KEK derived from the passkey's PRF output. */
export async function wrapWithPasskey(seed: Uint8Array, ref: PasskeyRef, given?: CredentialsLike): Promise<SeedEnvelope> {
  const prfSalt = random(SALT_BYTES);
  const prf = await evaluatePrf(ref, prfSalt, given);
  if (!prf) throw new PasskeyError("prf-unsupported", "the authenticator returned no PRF output");
  const hkdfSalt = random(SALT_BYTES);
  const key = await kekFromPrf(prf, hkdfSalt, ref);
  return seal(key, seed, {
    v: 1,
    kind: "passkey-prf",
    credentialId: ref.credentialId,
    rpId: ref.rpId,
    prfSalt: toB64u(prfSalt),
    hkdfSalt: toB64u(hkdfSalt),
    iv: toB64u(random(IV_BYTES)),
  });
}

/** Wraps `seed` under a PBKDF2-SHA256 key from `password`. */
export async function wrapWithPassword(seed: Uint8Array, password: string, iterations = PBKDF2_ITERATIONS): Promise<SeedEnvelope> {
  checkIterations(iterations);
  const salt = random(SALT_BYTES);
  const key = await kekFromPassword(password, salt, iterations);
  return seal(key, seed, { v: 1, kind: "password", iterations, salt: toB64u(salt), iv: toB64u(random(IV_BYTES)) });
}

export interface ProtectOptions {
  passkey?: PasskeyRef;
  password?: string;
  credentials?: CredentialsLike;
  iterations?: number;
  /**
   * Must be true for a passkey that fails PRF to fall back to the password.
   * Default refuse: a downgrade (old browser, or a page shim that hides PRF)
   * would otherwise store the seed password-only without the user choosing it.
   */
  allowPasswordFallback?: boolean;
}

/**
 * Entry point: the passkey when its PRF works; the password only when no
 * passkey is given, or when `allowPasswordFallback` is set. Refuses rather than
 * store a seed with neither.
 */
export async function protectSeed(seed: Uint8Array, opts: ProtectOptions): Promise<SeedEnvelope> {
  if (opts.passkey) {
    try {
      return await wrapWithPasskey(seed, opts.passkey, opts.credentials);
    } catch (e) {
      const fallback = e instanceof PasskeyError && e.kind === "prf-unsupported" && opts.password !== undefined && opts.allowPasswordFallback === true;
      if (!fallback) throw e;
    }
  }
  if (opts.password !== undefined) return wrapWithPassword(seed, opts.password, opts.iterations);
  throw new PasskeyError("no-method", "a passkey with PRF support or a password is required");
}

/** Decrypts an envelope. The returned seed is the caller's to zero after use. */
export async function unlockSeed(env: SeedEnvelope, opts: { password?: string; credentials?: CredentialsLike }): Promise<Uint8Array> {
  if (env?.v !== 1) throw new PasskeyError("bad-envelope", "unknown envelope version");
  if (env.kind === "passkey-prf") {
    const prf = await evaluatePrf(env, fromB64u(env.prfSalt), opts.credentials);
    if (!prf) throw new PasskeyError("prf-unsupported", "the authenticator returned no PRF output");
    return open(await kekFromPrf(prf, fromB64u(env.hkdfSalt), env), env);
  }
  if (env.kind === "password") {
    if (opts.password === undefined) throw new PasskeyError("no-method", "this envelope needs its password");
    checkIterations(env.iterations);
    return open(await kekFromPassword(opts.password, fromB64u(env.salt), env.iterations), env);
  }
  throw new PasskeyError("bad-envelope", "unknown envelope kind");
}
