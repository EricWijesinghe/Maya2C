---
title: Passkey unlock
description: How a wallet can use a device passkey to unlock its post-quantum key, what that protects against, and what it does not. Wallet side only; the chain is unchanged.
---

## In plain words

Your Maya2C key is a **post-quantum** key. It is stored on your device inside
an encrypted box. Until now the box opened with a password. With passkey
unlock, it can open with your fingerprint, face or device PIN instead.

The passkey is only the **key to the box**. It never signs a payment. Every
transaction is still signed by the post-quantum key inside the box, with the
same two signatures as before (ML-DSA-65 and SLH-DSA). The chain does not know
a passkey was used.

If your device cannot do this, the wallet uses a password instead, and says so.

## Threat model, stated exactly

- **What the chain sees:** ML-DSA-65 + SLH-DSA-SHA2-128s signatures, nothing
  else. No P-256 key or signature is ever sent to the chain, and no new
  signature suite is added (invariants 29 and 30 are untouched).
- **What the passkey does:** the WebAuthn **PRF** extension (CTAP2
  `hmac-secret`) makes the authenticator compute a secret from a value that
  never leaves the authenticator. HKDF-SHA256 turns that secret into an AES-256-GCM
  key-encryption key, and that key decrypts the post-quantum seed in memory.
- **A quantum attacker who breaks P-256** still needs an authenticator that
  holds the passkey. Breaking P-256 needs the public key, which is never
  published. And even the passkey's private signing key does not give the PRF
  output, which comes from a separate secret held by the authenticator.
- **Where that secret lives:** a device-bound passkey (a hardware security
  key, some platform authenticators) keeps it on that one device. A **synced**
  passkey (iCloud Keychain, Google Password Manager, Windows) copies it to
  every device signed in to that account and to the provider's sync store, so
  anyone who controls the account can unlock the seed. A website cannot forbid
  syncing; choose a hardware security key if you want one device only.
- **Not protected:** malware running in the wallet while the seed is unlocked;
  a weak fallback password (PBKDF2 with 600,000 iterations slows guessing, it
  does not stop it); a device someone else can unlock.
- **Lost device:** a passkey that is gone cannot unlock anything. Keep an
  **offline seed backup** as recovery. Do not keep a password envelope next to
  the passkey one: the seed is then only as safe as the password, because
  anyone who copies the stored envelopes can guess the password offline.
- **Password fallback is never silent:** if a passkey cannot produce PRF
  output, the wallet refuses unless the user chose the password route
  (`allowPasswordFallback`).

## For engineers

The module is `sdks/sdk-js/src/passkey.ts`, exported from `maya2c.js`:

```ts
import { registerPasskey, protectSeed, unlockSeed } from "maya2c.js";

const { ref, prfEnabled } = await registerPasskey({
  rp: { id: location.hostname, name: "My wallet" },
  user: { id: crypto.getRandomValues(new Uint8Array(16)), name: "me", displayName: "Me" },
});

// Passkey if its PRF works, otherwise the password. The envelope is safe to store.
const envelope = await protectSeed(seed, { passkey: ref, password });

const unlocked = await unlockSeed(envelope, { password });
// ... sign with the wasm SDK, then:
unlocked.fill(0);
```

- `envelope.kind` is `"passkey-prf"` or `"password"`; show the user which.
- Every envelope field except the ciphertext is AES-GCM additional data, so a
  changed credential id, relying party or salt fails to decrypt.
- A wrong passkey, wrong password or modified envelope all fail the same way:
  `PasskeyError` with kind `unlock-failed`. Error messages never contain key
  bytes.
- The module zeroes every buffer it owns before returning. JavaScript cannot
  promise that the engine made no copy; the seed returned by `unlockSeed` is
  yours to zero.
- Tests: `sdks/sdk-js/test/passkey.test.ts`, with a mocked
  `navigator.credentials` whose PRF is HMAC-SHA256 under a per-device secret.
  No real authenticator is exercised by CI.
