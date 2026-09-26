# 8. Cryptographic suites — v0.1.0

| Suite | Standard | Parameters | Sizes (B) | Use |
|---|---|---|---|---|
| ML-DSA-65 | FIPS 204 | security category 3 | pk 1952, sig 3309 | transaction signature, lattice half |
| SLH-DSA-SHA2-128s | FIPS 205 | category 1, "small" | pk 32, sig 7856 | transaction signature, hash-based half |
| ML-KEM-768 | FIPS 203 | category 3 | ek 1184, ct 1088 | p2p handshake |
| BLAKE3 | BLAKE3 spec | 32-byte output | — | ids, addresses, state root |

- **CRY-1** Only ML-DSA-65 is compiled from FIPS 204 (invariant 4); a key or signature of another parameter set cannot be decoded. *(external vectors: crates/crypto-pq/tests/acvp_tests.rs)*
- **CRY-2** The hybrid signature is the ML-DSA-65 signature followed by the SLH-DSA-SHA2-128s signature, both over the same bytes (TX-3). *(external vectors: crates/node/tests/suite_parity_tests.rs)*
- **CRY-3** Only parameter sets named in the signature-suite registry are compiled; an unknown suite byte is a decode error, never a fallback (invariant 29). *(external vectors: crates/crypto-pq/src/suite/tests.rs)*

Evidence outside `spec/tests/`: `crates/crypto-pq/tests/acvp_tests.rs`
(NIST ACVP vectors), `kem_kat_tests.rs`, and `spec/tests/keys.json`, whose
addresses both the reference and the TypeScript verifier recompute.

Deprecation lifecycle and hash agility: [docs/CRYPTO_WATCH.md](../docs/CRYPTO_WATCH.md).
