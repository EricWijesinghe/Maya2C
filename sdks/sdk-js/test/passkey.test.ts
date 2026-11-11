// P-17: the passkey only unlocks the PQ seed. A mocked authenticator stands in
// for navigator.credentials; its PRF is HMAC-SHA256 under a per-device secret,
// which is what CTAP2 hmac-secret computes.
import { describe, expect, it } from "vitest";
import {
  type CredentialsLike,
  PasskeyError,
  type SeedEnvelope,
  protectSeed,
  registerPasskey,
  unlockSeed,
  wrapWithPasskey,
  wrapWithPassword,
} from "../src/passkey.js";

const FAST = 1_000; // PBKDF2 iterations for tests only; the default is 600,000

function mockAuthenticator(deviceSecret: Uint8Array, prf = true) {
  const calls: unknown[] = [];
  const credentials: CredentialsLike = {
    async create(options) {
      calls.push(options);
      return {
        rawId: new Uint8Array([1, 2, 3, 4]).buffer,
        getClientExtensionResults: () => (prf ? { prf: { enabled: true } } : {}),
      };
    },
    async get(options) {
      calls.push(options);
      const first = (options as { publicKey: { extensions: { prf: { eval: { first: Uint8Array } } } } }).publicKey
        .extensions.prf.eval.first;
      if (!prf) return { getClientExtensionResults: () => ({}) };
      const key = await crypto.subtle.importKey("raw", new Uint8Array(deviceSecret), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
      const out = await crypto.subtle.sign("HMAC", key, first);
      return { getClientExtensionResults: () => ({ prf: { results: { first: out } } }) };
    },
  };
  return { credentials, calls };
}

const seed = () => Uint8Array.from({ length: 32 }, (_, i) => i + 1);
const deviceA = new Uint8Array(32).fill(0xa1);
const deviceB = new Uint8Array(32).fill(0xb2);

describe("passkey unlock (P-17)", () => {
  it("registers a passkey with PRF requested and user verification required", async () => {
    const { credentials, calls } = mockAuthenticator(deviceA);
    const { ref, prfEnabled } = await registerPasskey({
      rp: { id: "wallet.example", name: "Wallet" },
      user: { id: new Uint8Array([9]), name: "eric", displayName: "Eric" },
      credentials,
    });
    expect(prfEnabled).toBe(true);
    expect(ref).toEqual({ credentialId: "AQIDBA", rpId: "wallet.example" });
    const pk = (calls[0] as { publicKey: Record<string, unknown> }).publicKey;
    expect(pk.extensions).toEqual({ prf: {} });
    expect(pk.authenticatorSelection).toMatchObject({ userVerification: "required" });
  });

  it("PRF output wraps and unwraps the same seed", async () => {
    const { credentials } = mockAuthenticator(deviceA);
    const ref = { credentialId: "AQIDBA", rpId: "wallet.example" };
    const env = await wrapWithPasskey(seed(), ref, credentials);
    expect(env.kind).toBe("passkey-prf");
    expect(JSON.stringify(env)).not.toContain(Buffer.from(seed()).toString("base64url"));
    await expect(unlockSeed(env, { credentials })).resolves.toEqual(seed());
  });

  it("a different authenticator's PRF output does not unlock", async () => {
    const ref = { credentialId: "AQIDBA", rpId: "wallet.example" };
    const env = await wrapWithPasskey(seed(), ref, mockAuthenticator(deviceA).credentials);
    const err = await unlockSeed(env, { credentials: mockAuthenticator(deviceB).credentials }).catch((e) => e);
    expect(err).toBeInstanceOf(PasskeyError);
    expect((err as PasskeyError).kind).toBe("unlock-failed");
  });

  it("a modified envelope header does not unlock", async () => {
    const { credentials } = mockAuthenticator(deviceA);
    const env = await wrapWithPasskey(seed(), { credentialId: "AQIDBA", rpId: "wallet.example" }, credentials);
    const tampered = { ...env, rpId: "evil.example" } as SeedEnvelope;
    await expect(unlockSeed(tampered, { credentials })).rejects.toMatchObject({ kind: "unlock-failed" });
  });

  it("falls back to the password when the authenticator has no PRF", async () => {
    const { credentials } = mockAuthenticator(deviceA, false);
    const ref = { credentialId: "AQIDBA", rpId: "wallet.example" };
    const env = await protectSeed(seed(), { passkey: ref, password: "correct horse", credentials, iterations: FAST });
    expect(env.kind).toBe("password");
    await expect(unlockSeed(env, { password: "correct horse" })).resolves.toEqual(seed());
    await expect(unlockSeed(env, { password: "wrong" })).rejects.toMatchObject({ kind: "unlock-failed" });
  });

  it("refuses when there is no PRF and no password", async () => {
    const { credentials } = mockAuthenticator(deviceA, false);
    const ref = { credentialId: "AQIDBA", rpId: "wallet.example" };
    await expect(protectSeed(seed(), { passkey: ref, credentials })).rejects.toMatchObject({ kind: "prf-unsupported" });
    await expect(protectSeed(seed(), {})).rejects.toMatchObject({ kind: "no-method" });
  });

  it("prefers the passkey when PRF works", async () => {
    const { credentials } = mockAuthenticator(deviceA);
    const ref = { credentialId: "AQIDBA", rpId: "wallet.example" };
    const env = await protectSeed(seed(), { passkey: ref, password: "pw", credentials, iterations: FAST });
    expect(env.kind).toBe("passkey-prf");
  });

  it("does not modify the caller's seed and rejects unknown envelopes", async () => {
    const s = seed();
    const env = await wrapWithPassword(s, "pw", FAST);
    expect(s).toEqual(seed());
    await expect(unlockSeed({ ...env, v: 2 } as unknown as SeedEnvelope, { password: "pw" })).rejects.toMatchObject({
      kind: "bad-envelope",
    });
  });
});
