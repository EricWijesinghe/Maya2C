# Confidential federated training

**Status: RESEARCH.** Crate `confidential-ai`; tests in the crate and in
`tests/federated_ai_tests.rs`. Off-chain: it writes no chain state, and the
node takes it as a dev-dependency only (invariant 20).

## The brief, and what it maps to

| Asked | Built | Why |
|---|---|---|
| Hardware proofs of execution in SGX / SEV-SNP via `tee-attestation` | An `AttestationVerifier` trait binding a report to the round transcript; `NoAttestation` returns `Unattested`, which `require_attested` refuses. **No report is generated or verified** | No crate is named `tee-attestation`. Generating a report needs the hardware, and this host has none. Verifying one needs recorded reports and vendor chains, and none are in the tree (`sev` 8.0 and `dcap-qvl` 0.6 are the candidates) |
| Gradients encrypted with ML-KEM and averaged *inside enclaves* | Secure aggregation (Bonawitz et al., 2017) with ML-KEM-768 key agreement: pairwise masks cancel in the sum, and self-masks are Shamir-shared so up to `n − t` dropouts are recovered | This keeps privacy independent of any vendor (below). The aggregator learns only the sum |
| ε-differential privacy that prevents reconstruction | Clipping, stochastic quantization, integer discrete-Gaussian noise; a zCDP accountant reporting (ε, δ) | Differential privacy *bounds* what the weights reveal about one participant; it does not prevent reconstruction. The accountant says what the bound is |
| 10 nodes fine-tune without revealing samples | `tests/federated_ai_tests.rs`: exact masked sums, dropout recovery, threshold refusal, tamper detection, no sample bytes in transcripts, a model trained to ≥ 85% held-out accuracy with ε reported | — |

## Why enclaves are a hook, not the root of trust

An SGX DCAP quote chains to Intel's root key and a SEV-SNP report to AMD's,
both ECDSA (P-256 and P-384). On this chain that is:

- **a trusted party** — the reason the architecture vision flagged TEE
  attestation as colliding with invariant 11, which requires introducing the
  chain's only trusted party to be a decision somebody writes down;
- **not post-quantum** — a quantum attacker forges the vendor signature;
- **hardware with a record of side-channel breaks** — SGX in particular.

So the privacy of an update rests on masking and noise, which need no vendor.
An attestation could add one claim on top — that aggregation ran as a measured
binary — once a real verifier and test vectors exist. Nothing writes chain
state, so invariant 11 is not engaged. Committing an attested round to state
would engage it, and that is a written decision first.

## Secure aggregation

Per round, `n` participants, threshold `t` with `2t > n`:

1. Each publishes an ML-KEM-768 encapsulation key and a BLAKE3 commitment to a
   fresh self-mask seed.
2. For each pair `i < j`, `i` encapsulates to `j`. The shared secret gives a pair
   seed and one ChaCha20-Poly1305 key per direction, all bound to the round.
3. Each Shamir-shares its seed over GF(2^8) at threshold `t`, sealing one share
   to each peer.
4. Each sends `update + self-mask + Σ_{j>i} mask_ij − Σ_{j<i} mask_ij` modulo 2^32.
5. The aggregator names the survivors. Each survivor reveals its shares of the
   survivors' seeds and its pair seeds with the dropped. Every reconstructed
   seed is checked against its commitment, because interpolating wrong shares
   returns a different secret silently (invariant 19's lesson).

A participant answers for one survivor set only and never reveals a self-seed
share for a peer it was told dropped, so it never releases both halves for
anyone.

The sum is taken modulo 2^32 and decoded as signed, so a round carries a
per-coordinate input bound: `RoundConfig::new` refuses one where
`participants × bound` could reach 2^31, and `masked_input` refuses any
coordinate past it. A wrapped sum would be a silently wrong aggregate, and the
privacy accounting is stated in its units.

**Not covered.** The model is an aggregator that follows the protocol, with
fewer than `t` colluding participants. An aggregator that tells different
participants different survivor sets needs the original protocol's signed
consistency round. A survivor dropping *during* unmasking aborts the round.

## Differential privacy

- **Sensitivity.** One participant's clipped update, scaled, plus rounding:
  `Δ = ⌈C·s⌉ + ⌈√d⌉` quantized units.
- **Noise.** Discrete Gaussian `N_Z(0, σ²)` per coordinate, from the
  Canonne–Kamath–Steinke exact rejection sampler over integers. There is no
  floating-point noise to leak (Mironov 2012).
- **Guarantee.** `ρ = Δ²/(2σ²)` zCDP per round, additive over rounds, and
  `ε = ρ + 2√(ρ·ln(1/δ))` (Bun–Steinke 2016).
- **Who adds it.** Every participant adds the whole target on its own. That is
  exact (other participants' noise is post-processing) and conservative: `n`
  times the noise a trusted aggregator would need. Tighter accounting for sums
  of discrete Gaussians (Kairouz–Liu–Steinke 2021) is the known improvement.

**The honest number.** The test trains ten nodes for 30 rounds at `ρ = 4` per
round and reaches ≥ 85% accuracy, at a total ε above 100 for δ = 10⁻⁵. That is
a weak guarantee. Ten participants with conservative per-node noise cannot buy
a small ε and a useful model at once. The ways out are many more participants
(noise averages over `n`), the tighter distributed accounting, fewer rounds, or
accepting lower accuracy.

## Not done

- SGX / SEV-SNP report generation and verification.
- The signed survivor-consistency round (malicious aggregator).
- Recovery from dropouts during unmasking.
- Distributed discrete-Gaussian accounting.
- Any chain integration. A round commitment in state would be a written
  invariant-11 decision.
