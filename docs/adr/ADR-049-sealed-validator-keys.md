# ADR-049: Sealed validator keys at rest

**Status:** Accepted, 2026-10-18
**Context owner:** Eric Wijesinghe (Phase-1 security plan, approved 2026-10-17)

## Context

An audit on 2026-10-17 found two problems on the live testnet:

- Each of the four validators keeps its ML-DSA-65 consensus key as **plaintext hex**.
- Every local account could read those files, and BitLocker is off on both volumes.

Anyone who copied one file could sign as that validator. Permissions were tightened by hand on the same day, but permissions do not survive:

- a copied backup,
- a disk taken out of the machine,
- a mistaken `icacls`.

`crates/signer/src/keystore.rs` already seals a 32-byte master seed (ADR-022). A node's validator key is the *expanded* 4,032-byte ML-DSA-65 secret key, written by `--generate-validator-key`. It is not derived from a seed, so existing keys cannot be moved into that format.

## Decision

1. **A second sealed format, `crates/signer/src/sealed.rs`.**
   - It seals any secret byte string with the keystore's primitives:
     - Argon2id, 64 MiB, 3 passes;
     - ChaCha20-Poly1305, with a random 16-byte salt and 12-byte nonce.
   - Every cleartext field is bound in as length-prefixed associated data: format, kind, KDF parameters, salt and public key. So swapping the public key, cheapening the KDF or relabelling the kind fails the tag, exactly like a wrong passphrase does.
   - Two bounds:
     - KDF parameters above 1 GiB or 64 passes are refused *before* hashing, so a crafted file cannot exhaust the host.
     - Passphrases under 12 bytes are refused when sealing.
2. **The node reads the passphrase from a file.** It comes from `--validator-passphrase-file` or from the file named by `MAYA2C_VALIDATOR_PASSPHRASE_FILE`. Never from argv or an environment value: both are visible to any process that can list this one.
3. **New and migrated keys.**
   - `--generate-validator-key` seals when a passphrase file is set.
   - `--seal-validator-key <IN> <OUT>` migrates an existing key. It refuses to overwrite `OUT`, and it reopens `OUT` and compares public keys before reporting success.
4. **Plaintext keys:**
   - **A `production` build refuses them.** A mainnet validator cannot start with a readable consensus key.
   - **Devnet and testnet builds still load them,** with a warning, so the running testnet is not broken by an upgrade.
5. **The wallet** (`l1-wallet register-validator`) opens both formats, using the same kind label and passphrase variable.

## Consequences

- **Moving the testnet:** each operator runs `--seal-validator-key`, restarts with the passphrase flag, and then destroys the plaintext file. That is a restart of each validator, done one at a time and never during a halt.
- **The passphrase file is the new secret.**
  - It belongs **off the key's volume** or in OS-protected storage.
  - Next step: Windows DPAPI or a TPM-sealed passphrase. Until then, a passphrase file stored next to the key only protects against copies of the key alone.
- **Not covered:**
  - **Memory.** The key is in process memory while the node runs; zeroized buffers limit stray copies but do not stop a debugger. The remote signer (ADR-033) remains the stronger option for a validator that can run one.
  - **No external review.** The format has unit tests for every tamper case, but it has not been externally reviewed.

## Tests

- `crates/signer/src/sealed.rs` covers:
  - round trip;
  - the secret absent from the file;
  - wrong passphrase;
  - swapped public key;
  - weakened KDF;
  - flipped ciphertext bit;
  - wrong kind;
  - oversized KDF memory;
  - short passphrase.
- `bins/maya2c-node/src/validator_key.rs` covers:
  - generate and load;
  - sealed key without a passphrase;
  - wrong passphrase;
  - seal preserving the key and refusing overwrite;
  - CRLF passphrase files;
  - devnet plaintext;
  - production refusal.
- `bins/l1-wallet/src/staking.rs` covers a sealed key loading only with its passphrase.
