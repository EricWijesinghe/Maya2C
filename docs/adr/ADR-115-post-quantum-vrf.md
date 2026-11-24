# ADR-115: Post-quantum randomness for the beacon — a hash-chain scheme now, a lattice VRF behind a research gate

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`) for scheme 2; scheme 3 has no height until its gate is met.
**Date:** 2026-11-12
**Related:** parity row P-32, CATALOG row `pq-randomness`, [prior-art/post-quantum-vrf.md](../prior-art/post-quantum-vrf.md), [ADR-024](ADR-024-oracle-choice.md), [ADR-007](ADR-007-signature-suites.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), `docs/oracle.md`; invariants 8, 10, 11, 12, 25, 29, 30, 33

## Context

`crates/vrf` is RFC 9381 `ECVRF-EDWARDS25519-SHA512-TAI`, "a classical
primitive on a post-quantum chain", stated first in its own docs
(`crates/vrf/src/lib.rs:1-26`): "There is no standardized, practical
post-quantum VRF today", a break "costs future unpredictability, not past
outputs", and every key carries a scheme tag so a second VRF lands "exactly
as SLH-DSA landed beside ML-DSA" (`crates/vrf/src/scheme.rs:1-18`,
`VrfScheme` `:39`, `tag()` `:48`, `suite_octet()` `:81`; `VrfPublicKey`
encodes as `tag ‖ 32 bytes`, `keys.rs:156`). Suite octet `0x03` is
invariant 10.

Its consumer is the oracle's randomness beacon:
`alpha_n = previous_beacon ‖ height_n; output_n = VRF(sk_proposer, alpha_n);
beacon_n = H(previous_beacon ‖ output_n)` (`crates/node/src/oracle/beacon.rs:5-9`),
with the proposer chosen from the registry by the previous beacon
(`registry.rs:161`, `beacon_proposer`). The registry stores
`address ‖ scheme_tag ‖ vrf_pk` per authority (`AUTHORITY_LEN = 32 + 1 + 32`,
`registry.rs:45`). A miner may include or omit the proposer's proof and
gets "one bit of influence over each block it mines" (`beacon.rs:23-35`).
Contracts read it as `block_randomness` (`crates/vm/src/runtime.rs:855`),
the previous block's value. The oracle, and so the beacon, is optional and
absent by default (invariant 11).

**Finding that contradicts the brief.** P-32 reads "PQ VRF replacing ECVRF
in committee selection". No VRF selects the committee. The active set is
every eligible validator ranked by stake, largest first, ties by id
(`crates/staking/src/types.rs:175-187`, `max_validators = 100`,
`params.rs:19,42`), and the DAG's anchor per round is deterministic
(`crates/dag-bft/src/commit.rs:88-93`; `proposer_of` in
`bins/maya2c-node/src/bft.rs:961` is a hash of the sender). ECVRF is used
for the **beacon**, and only when an oracle registry exists. This ADR is
about the beacon; random committee sampling is a different consensus rule
that nobody has proposed, and it would need its own ADR and invariant
review (10, 12).

### What the literature has (survey, 2026-11-12; prior-art file has links)

| Construction | Assumption | Shape | Status |
|---|---|---|---|
| LB-VRF (ePrint 2020/1222, FC 2021) | Module-SIS/LWE | **few-time**: a key is good for a bounded number of evaluations | academic |
| X-VRF (ePrint 2021/302, ESORICS 2022) | hash functions (XMSS) | many-time, **stateful**; the chain's state tracks the XMSS index to keep outputs unique | academic |
| LaV (ePrint 2022/141, CRYPTO 2023) | Module-SIS/LWE | long-term; the abstract claims the shortest outputs among standard-assumption PQ VRF proposals | academic; no second implementation found |
| DeuringVRF (Eurocrypt 2025) | isogenies | long-term | academic; young assumptions, not fetched this round |
| Brandt (ePrint 2025/766) | generic | a transform to strong unbiasability in the standard model | academic |
| RFC 9381 | ECDLP | ECVRF, what `crates/vrf` is | **the only VRF standard found** |

No NIST, IETF CFRG or ISO post-quantum VRF standard or draft was found
(search terms in the prior-art file). ML-DSA and SLH-DSA are **not** VRFs:
a signer can produce more than one valid signature for a message and a
verifier cannot tell, so `H(signature)` is grindable by the signer. A
deterministic signing mode fixes the signer's algorithm, not the verifier's
knowledge.

## Decision

**Scheme tag 2 is a hash-chain commit-reveal scheme built from BLAKE3 alone,
with the withholding bias stated and bounded. Scheme tag 3 is reserved for
a lattice VRF and is not compiled until the research gate below is met.
The beacon folds every scheme's contribution, so the mix is at least as
unpredictable as its strongest live component.**

### Scheme 2: `HASH-CHAIN-BLAKE3`

- Key generation: a 32-byte seed `s`; chain `r_N = s`, `r_i = H("maya2c/vrf/2/link" ‖ r_{i+1})`
  for `i = N-1 … 0`; the public key is `r_0`, with `N = CHAIN_LEN = 2^20`.
  Secret material is the seed and the index, typed and zeroized (Standing
  Order 5).
- Registry record v2 per authority: `address ‖ 0x02 ‖ r_0 ‖ next_index(4)`.
  The index is committed state under `o:` (invariant 25, undo covered),
  which is X-VRF's "blockchain-managed state" idea applied to a chain
  instead of a tree.
- Prove for `alpha`: reveal `r_{next_index}`; the proof is 32 bytes. Verify:
  `H^{1}(r_i) == r_{i-1}` against the stored link, then `next_index += 1`.
  Output: `H("maya2c/vrf/2/out" ‖ pk ‖ alpha ‖ r_i)`, 64 bytes
  (`OUTPUT_LEN`, `scheme.rs:34`).
- **Uniqueness**: one preimage per link, so exactly one valid proof per
  index; a wrong index is refused (stale or skipped links are not accepted,
  a skipped link would let the holder choose among several).
- **Unpredictability**: nobody but the holder knows `r_i` before it is
  revealed. The holder knows all of its own future links; it cannot choose
  `alpha` (both halves fixed, `beacon.rs:14-21`).
- **The known weakness, stated**: the holder can compute its own
  contribution ahead and decide to withhold (fall back, `beacon.rs:25-27`).
  That is the one bit the miner already has; scheme 2 gives the proposer
  the same bit. Over the accumulator (`beacon.rs:32-35`) a bit now is
  composed with every future contribution. Scheme 2 is **weaker** than a
  true VRF against a holder; it is **stronger** than ECVRF against a
  quantum adversary, which recovers nothing from `r_0`.
- Exhaustion: at `next_index == CHAIN_LEN` the authority is skipped by
  `beacon_proposer` and must rotate in a new chain (`RotateAuthorities`).
  `2^20` links is about 12 days of being proposer every second; the
  registry has 3 to 21 authorities, so in practice years.

### The mixed beacon

`beacon_n = H(previous ‖ output_n)` is unchanged in shape. Each authority
registers one key per scheme it holds, and the proposer's proof carries
one proof per scheme it registered: `output_n = H(out_scheme1 ‖
out_scheme2 ‖ …)` in tag order. A proposer registered under both ECVRF and
scheme 2 must supply both; one bad proof refuses the submission. An
adversary that breaks ECVRF learns `out_scheme1` early and still not
`out_scheme2`; the fold is unpredictable as long as one component is. This
is CATALOG `pq-randomness` phase 1.

### Scheme 3: reserved for a lattice VRF (research gate)

`VrfScheme::Lattice = 0x03` is reserved in `scheme.rs` and refused by
`from_tag` until **all** of the following are in `reports/`:

1. A security proof in a peer-reviewed venue plus at least one independent
   cryptanalysis paper on the same construction, both cited by ePrint id.
2. Two independent implementations agreeing on published test vectors, one
   of them this tree's, pinned the way `rfc9381_vectors.rs` pins ECVRF.
3. Proof size and verification time measured on this hardware, with commit
   and command, against `MAX_BLOCK_BYTES` (invariant 33) and the block
   target.
4. Constant-time evidence (dudect, as for HQC in `features.toml`).
5. An amendment to this ADR with the activation height.

Until then, scheme 3 is a tag the decoder refuses, so a key some nodes
accept and others do not cannot exist (the reasoning of invariant 29,
applied to `VrfScheme` rather than `SuiteId`). LaV is the leading
candidate on the survey; X-VRF is the fallback if stateful hash-based
keys prove acceptable.

## Alternatives considered

- **Replace ECVRF outright with scheme 2.** Loses a true VRF's resistance
  to withholding by the holder; the mix keeps it while ECVRF stands.
- **`H(ML-DSA signature)` as a VRF.** Not unique; see above. Rejected.
- **Ship LaV now.** One implementation, no independent cryptanalysis found,
  no measured proof size on this hardware. The gate exists so this is a
  written decision, not a mood.
- **Commit-reveal with a fresh commitment per round (RANDAO).** Needs two
  transactions per contribution and a slashing rule for non-reveal; the
  hash chain pre-commits every reveal at registration, so one submission
  per round, as today.
- **A threshold or VDF beacon.** No post-quantum VDF with a standard; the
  sealed-mempool committee is a different trust set (CATALOG P-41).

## Consequences

- With an oracle configured, the beacon becomes unpredictable to a
  quantum adversary at the cost of one extra 32-byte reveal per block and
  one index write.
- A chain without an oracle is unchanged (invariant 11).
- The registry record grows by scheme and index; rotation vectors change.
- `crates/vrf` gains a scheme that is not a VRF in the RFC sense; its docs
  say so at the top, as `lib.rs:10-26` does for the classical one.
- Random committee sampling, if ever wanted, has a PQ source to draw from.

## Invariants touched

- **10**: `0x03` and the RFC vectors untouched; scheme 1 is unchanged.
- **11**: nothing without a registry.
- **12**: the scheme list is compiled; governance chooses nothing here.
- **25**: registry record v2 under `o:`; **8**: undo covered.
- **29**, **30**: not a signature suite; `VrfScheme` is the parallel
  registry, and the refusal-of-unknown-tag rule is the same.
- **33**: 32-byte proofs; scheme 3's size is a gate item.

## Activation

`FeatureSet::PQ_VRF = 1 << 23` (claimed in `adr-claims.txt`; VRF scheme
tags 2 and 3 also claimed), compiled `PQ_VRF_ACTIVATION_HEIGHT = u64::MAX`,
scheduled under ADR-082 with the v4 bundle at **genesis + 15,552,000** for
scheme 2 and the mixed beacon. Before activation a scheme-2 key in a
rotation is refused. Scheme 3: no height.

## Test plan

Red tests, `crates/vrf/tests/hash_chain_tests.rs` and
`crates/node/tests/pq_beacon_tests.rs`:

- `a_reveal_verifies_against_its_link_and_a_skipped_link_is_refused`
- `a_reused_link_is_refused_by_the_index`
- `the_mixed_output_changes_when_either_components_proof_changes`
- `a_proposer_registered_under_two_schemes_must_supply_both`
- `an_exhausted_chain_is_skipped_by_beacon_proposer`
- `scheme_tag_3_is_refused_by_from_tag`
- `a_reorg_restores_the_registry_index`
- `the_rfc9381_vectors_still_pass` (invariant 10, unchanged file)

Vectors (`spec/08-crypto.md` VRF-2-1..VRF-2-4): a chain of length 8 from a
fixed seed; two consecutive reveals; the mixed output with a fixed ECVRF
vector; a negative vector with a skipped link. The withholding bias is
exercised by `chaos_simulator.rs`: a proposer that withholds whenever the
fallback favours it, and the measured skew reported with the run.

## Open questions for Eric (economics and pricing only)

1. Whether the beacon proposer earns anything per contribution (today
   nothing); a reward changes the value of the withholding bit.
2. No other economics here. The security choice (scheme 2's stated
   weakness, the gate for scheme 3) changes the protocol's security model
   and is flagged under "When to stop and ask Eric".
