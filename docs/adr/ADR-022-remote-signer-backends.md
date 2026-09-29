# ADR-022: Remote signer — keystore backend now, HSM/KMS when one can be tested

**Status:** Accepted
**Date:** 2026-09-26

> **Update 2026-09-28:** DAG-BFT is now in the node, and since ADR-033 (2026-09-29) it can sign through this remote signer with `--remote-signer`. ADR-033 also replaced the protection rule below, which would have stalled consensus. See [ADR-027](ADR-027-dag-bft-in-the-node.md). The text below is the decision as recorded; it is not current status.

## Context

Master Prompt 16 §1: validator consensus keys must not live on the
validator host. A separate signer process holds them. Backends asked for: an
encrypted keystore (dev), HSMs via PKCS#11, and cloud KMS. Which HSM and KMS
vendors support ML-DSA (FIPS 204), and at what certification level, must be
recorded here.

## Vendor landscape

> Written from knowledge current to mid-2026. **Not checked live**: no vendor
> site or NIST CMVP listing was consulted in this session. Treat every cell as
> something to re-verify before choosing a vendor.

| Vendor / service | ML-DSA as understood | Certification caveat |
|---|---|---|
| AWS KMS | ML-DSA key specs announced in 2025 | KMS HSMs are FIPS 140-3 validated; whether a given validation covers the ML-DSA module version must be checked in the CMVP listing |
| Google Cloud KMS | quantum-safe signatures (ML-DSA, SLH-DSA) offered, initially as preview | preview features carry no SLA; check GA status |
| Thales Luna, Entrust nShield, Utimaco | PQC firmware/option packs with ML-DSA announced | FIPS 140-3 validation of the PQC-capable firmware lagged the firmware; CAVP algorithm certificates are not module validations |
| SoftHSM (open source) | PKCS#11 v3.2 PQC mechanisms not supported in releases known to this session | test-only anyway |

PKCS#11 v3.2 defines ML-DSA mechanisms. Module support is the gating item,
and it varies by firmware version.

## Decision

1. **The encrypted keystore is the only backend built**:
   - Argon2id (64 MiB, 3 passes) seals a ChaCha20-Poly1305 key over the seed;
   - the seed is decrypted into a zeroizing `MasterSeed`;
   - the file carries the public key, and a mismatch after decryption is
     refused.

   For production it runs on **dedicated signer hardware**, not the validator
   host. The brief's fallback for when no PQ-capable HSM is available applies,
   and this ADR says so.
2. **PKCS#11 and KMS backends exist as named functions that refuse with the
   reason.** They do not pretend. No ML-DSA-capable PKCS#11 module was
   available here to integrate and test against. A KMS backend needs cloud
   credentials and a paid key, which Standing Order 6 reserves for explicit
   approval.
3. **Threshold signing** (m-of-n signer hosts) is not wired. The custody
   design exists (`crates/custody-mpc`, with `threshold-lattice` as
   RESEARCH), but a threshold ML-DSA signature is not a standard object. A
   2-of-3 *multisig* of signer hosts would change what a certificate
   verifies. That is a consensus change and gets a MIP, not a backend flag.
4. **The channel** is a SIGMA-style signed-KEM exchange:
   - ML-KEM-768 provides the shared secret;
   - ML-DSA-65 identities are pinned on both sides;
   - both signatures cover the transcript;
   - frames are ChaCha20-Poly1305 with counter nonces.

   It is **not externally reviewed**. Tests cover pinning, forgery and
   replay, not a proof.

## Consequences

- Slashing protection is enforced in the signer. The node cannot talk it
  out of a refusal. Records are fsync'd before a signature exists, and a
  damaged history makes the signer refuse to start rather than forget.
- Moving a validator between machines goes through the EIP-3076-style
  interchange (`5-maya2c`). An import keeps the union of both histories.
- Nothing in the node calls the signer yet. Consensus signing is not wired
  into the node at all, because DAG-BFT is not wired (ADR-015). The signer is
  ready for that integration, not integrated.
