# ADR-051: Moving to NIST security level 5, where and in what order

**Status:** Accepted for options 1 and 2 (Eric, 2026-10-18); 3 at mainnet genesis; 4 not now.

## Context

The Phase-1 security plan, approved 2026-10-17, asks for "PQ defaults to ML-DSA-87 / ML-KEM-1024", which is NIST category 5, the highest. Today:

| Where | Today | Level |
|---|---|---|
| Transaction signatures (default account suite) | Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s, both must verify | 3 + 1, two independent hardness assumptions |
| Account suites available now (ADR-007, ADR-013: envelopes active from height 0) | Includes ML-DSA-87 | 5, opt-in |
| Validator / BFT vote keys | ML-DSA-65 | 3 |
| P2P handshake (Noise) | ML-KEM-768, plus the HQC dual-KEM on default builds | 3 |

Sizes from FIPS 203 / 204, in bytes:

| | Public key | Signature / ciphertext |
|---|---|---|
| ML-DSA-65 | 1,952 | 3,309 |
| ML-DSA-87 | 2,592 | 4,627 |
| ML-KEM-768 | 1,184 | 1,088 |
| ML-KEM-1024 | 1,568 | 1,568 |

Level 5 costs bytes. Each signature is about 40% larger, which means larger blocks, higher fees and fewer transactions per block. This TPS cost has not been measured here: MISSING.

## Options, cheapest first

1. **P2P handshake to ML-KEM-1024.**
   - Gain: protects today's traffic against "record now, decrypt later".
   - Cost: about 900 extra bytes per connection, once. Not consensus.
   - Catch: all peers must speak it, so it ships as a negotiated protocol version (1024 preferred, 768 accepted until a cutoff), never as a flag day.
2. **Wallet "Maximum security" account option.** The suite already exists (ML-DSA-87). Only the wallet UI, SDK default flag and docs change. Users choose it and pay the larger fee.
3. **Validator and council keys at ML-DSA-87.** These keys are long-lived and few, so the extra bytes per vote matter less. But the BFT key type is fixed in genesis and vote certificates (ADR-021), so this is a consensus change: a spec update, conformance vectors, an ADR amendment, and either a re-genesis or an activation height.
4. **Default account suite to ML-DSA-87 + SLH-DSA-256.** This makes every transaction much larger. The current hybrid already rests on two independent assumptions (lattice *and* hash), which is the stronger protection against a cryptanalytic surprise. Raising both to level 5 is the most expensive option and the least needed.

## Recommendation

- Do **1** and **2** now. They are non-consensus and the testnet keeps running.
- Do **3** at mainnet genesis, since its keys are generated in the ceremony anyway. This needs Eric's approval because it changes the protocol.
- Do **not** do **4** now. Revisit it only if a lattice cryptanalysis result appears. ADR-013 already lets every account move without a fork.

## Decisions needed from Eric

1. Approve 1 and 2.
2. Approve 3 for mainnet genesis.
3. Confirm 4 stays opt-in.

## What option 1 shipped (2026-10-18)

- `/maya/mlkem1024/1.0.0` (ML-KEM-1024, `crates/node/src/network/pq/`) is offered
  first on every default build, with `/maya/mlkem/1.0.0` (ML-KEM-768) second.
  A 768-only peer still connects; the test `a_mlkem1024_node_falls_back_to_a_768_only_peer`
  pins that.
- Its own KDF context and transcript domain, so a 1024 session can never share
  keys with a 768 session. Message widths are 1 + 1568 each way.
- `MlKem1024Policy::Required` exists to drop the 768 fallback later. The default
  is `Preferred`. Operators set it with `--mlkem1024 <preferred|off|required>`.
- **The default stays `preferred` under `--features production`.** `required`
  would partition the network from every peer that has not upgraded, and no
  measurement yet shows the share of level-5 sessions. The flip to `required`
  is an operator decision, made once `pq_sessions_by_protocol` shows the
  network has moved. It is not a release-cut decision.
- Each handshake records its exchange (`pq_sessions_by_protocol{protocol=...}`),
  so the share of level-5 sessions is visible.
- The downgrade is real and is documented in `pq/mod.rs`: forcing 768 needs an
  active attacker against a live Noise session.

Option 2 (wallet "Maximum security" account) is not part of this change.

## Option 3 built (2026-10-28, Eric: "Build it now")

- Validator keys are their own type, `custom_l1_node::crypto::validator_keys` (ML-DSA-87),
  on `maya_crypto_pq::suite::validator` (the `ml-dsa` backend; invariant 4 unchanged:
  `fips204` still compiles only ML-DSA-65). Transaction keys stay ML-DSA-65 in the hybrid,
  so transactions do not grow.
- Where it applies: genesis `bft.validators`, staking `Register` (key + proof of possession),
  BFT vertex, vote, attestation and checkpoint signatures, the remote signer, the sealed
  validator keystore (the 32-byte seed is sealed, not the expanded key).
- Signatures are bound to the FIPS 204 context `maya2c validator v1`: a validator signature
  never verifies under the empty context transaction and suite keys use. Spec: CRY-4.
- Security-council keys are still ML-DSA-65 (`state/council.rs`); moving them is a follow-up.
- Consensus and wire change: every existing validator key and sealed key file is invalid.
  Re-genesis only, with keys regenerated by `maya2c-node` built from this change.
