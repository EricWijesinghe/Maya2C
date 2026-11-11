# ADR-125: Succinct chain-state proof — a STARK over the checkpoint chain, off-chain prover, with the recursion gap stated

**Status:** Proposed, 2026-11-12. Phases S1–S2 are off-chain tooling (W3/W4, no node change); phase S3 is post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-31, CATALOG row `succinct-state-proofs`, [prior-art/succinct-chain-proof.md](../prior-art/succinct-chain-proof.md), [ADR-008](ADR-008-transparent-zk.md), [ADR-038](ADR-038-attested-checkpoints.md), [ADR-040](ADR-040-stake-weighted-committee.md), ADR-089 (finality light client, branch `feat/light-client-finality`), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 22 (retired, still binds gas), 24, 26, 33

## Context

What a light client verifies today is a **checkpoint**: a block id with a
quorum of its epoch committee's ML-DSA-87 attestations
(`crates/node/src/consensus/bft/attest.rs:171-215`, `Checkpoint::verify`,
quorum n − f heads or W − f stake). ADR-089 M0 (branch
`feat/light-client-finality`, not on master) wraps that verifier in
`crates/light-client` and measured acceptance at n = 7: 10.8 ms, n = 100:
152.6 ms on one machine (`reports/bench/light-client-finality-2026-11-10.md`),
with an `EpochHandoff` carrying header and staking-record Merkle proofs. A
client following the chain verifies one checkpoint per epoch handoff plus
one per state it cares about; a bridge or a phone that was offline for
months verifies every handoff in between. P-31 asks for one proof that
stands for all of them.

The proof system is `crates/zk-stark`: Plonky3 uni-STARKs over BabyBear,
hiding FRI, **Keccak-256 commitments** (`config.rs:35-45`), pinned at
`p3-* = 0.7.0` (`Cargo.toml:14-28`), with a bytes-in, verdict-out API
(`proof.rs:72-105`) and Plonky3's own security estimator (`:110-135`).

**Findings that contradict the brief.**

1. **No recursion exists in this tree.** `crates/zk-stark` depends on no
   recursion crate; `p3-uni-stark` 0.7.0 proves one AIR over one trace. A
   "recursive STARK over checkpoints" needs a verifier circuit, and that
   does not exist here.
2. **Plonky3's recursion is not a dependency anyone can pin yet.** The
   upstream `Plonky3-recursion` project (prior-art file) is a fixed
   recursive verifier for `p3-uni-stark`/`p3-batch-stark` proofs; its own
   book says it is under active development, unaudited, not recommended
   for production, and not published on crates.io. Its backend is a
   Poseidon2 configuration. This tree's proofs are Keccak-committed:
   verifying one of them inside a circuit means Keccak in-circuit, which
   is the expensive case the recursion project avoids by choosing
   Poseidon2. A recursive path needs a **second config** (Poseidon2
   commitments; `hash.rs` already has Poseidon2 width 16 for in-circuit
   use) and that config has no proofs, vectors or security numbers today.
3. **The expensive statement is not the Merkle path, it is ML-DSA-87.** A
   checkpoint is n − f lattice signatures. No measurement of ML-DSA
   verification inside a STARK exists in this tree or in the prior-art
   search; the ADR does not guess one (Standing Order 2).
4. **Phase numbering.** ADR-089 names STARK aggregation "phase 2" and
   consensus-visible committee commitments "phase 3"; the CATALOG row says
   "research and an ADR before code" (W4). This is that ADR.

## Decision

**A chain-state proof is a STARK whose public inputs are a trusted anchor
checkpoint and a target checkpoint, and whose statement is "every epoch
handoff between them verifies under ADR-089's rules". It is produced
off-chain by anyone, verified by a light client with `zk_stark::verify`,
and the chain itself never verifies one. Recursion is a gate, not a
promise: until a Poseidon2 config and an in-circuit ML-DSA-87 verifier are
measured, the proof is one non-recursive STARK over a bounded number of
handoffs, and a client that needs more chains them by verifying each.**

### S1 — the statement, non-recursive (off-chain, W3)

One AIR, `HandoffChainAir`, over `K ≤ MAX_HANDOFFS_PER_PROOF` handoffs:

- Public inputs: `anchor = (epoch, block id, committee commitment)`,
  `target = (epoch, block id, committee commitment)`, `K`.
- Per handoff `i`: the committee of epoch `i` is the preimage of the
  committee commitment (a BLAKE3 tree of validator ids — the same leaf
  the finality client already ports, ADR-089 consequence 2); a quorum of
  those members signed `(epoch, height, block)`; the block's header hashes
  to `block`; the header's state root contains `k:cmt:<i+1>` and
  `k:cmw:<i+1>` by Merkle path; those records are the next committee and
  weights. Quorum is counted in weights when the anchor says the chain is
  weighted, as `Checkpoint::verify` does (`attest.rs:185-200`).
- Witness: the checkpoints, headers and record proofs. The prover is a
  full node or anyone who fetched `get_checkpoint` and the proofs
  (`crates/node/src/rpc/types.rs:130-180`).

The ML-DSA-87 gadget is the open cost. S1 therefore ships in two steps:
first with signature verification **outside** the circuit (the proof
covers the hashing and Merkle structure, the client still verifies the
n − f signatures of the *target* checkpoint natively, so the proof saves
the intermediate handoffs' structure checks but not their signatures),
then with the gadget once `reports/` holds its measured row count and
prover time on this hardware. The first step is honest about what it
saves and is where the vectors come from.

### S2 — recursion behind a gate (W4)

Recursion is adopted when all of the following are in `reports/`:

1. A Poseidon2-committed `Config2` in `crates/zk-stark` with
   `security_bits` output and a vector set parallel to the Keccak config.
2. An in-circuit verifier for `Config2` proofs with a measured trace size
   and prover time for one `HandoffChainAir` proof, on this hardware,
   with commit and command.
3. The upstream recursion code published with a version, an audit or two
   independent reviews cited by link, and `cargo vet`/`cargo deny` clean
   for it; **or** this tree's own verifier AIR, reviewed by two people.
4. Proof size at the target depth against what a phone verifies in the
   time ADR-089's research note sets (MISSING today).

Until then a client needing more than `MAX_HANDOFFS_PER_PROOF` epochs
verifies a sequence of S1 proofs; each one's `target` is the next one's
`anchor`. That is linear in epochs, not in blocks, and it is what the tree
can prove today.

### S3 — consensus-visible committee commitment (postgen/v4)

S1 recomputes the committee commitment from staking records by Merkle
path, which costs a path per member. S3 writes `k:cmc:<epoch>` — the
BLAKE3 root of the committee's ids and weights — into state at the epoch
boundary, so a handoff proof opens one leaf. Under the state root
(invariant 25), undo-covered, same batch as the block (invariant 26). This
is the single node change in this ADR, and it is dark until its height.
It changes nothing a full node checks; it adds a record a prover reads.

### What the chain never does

- Verify a chain-state proof. No host function, no transaction kind. A
  proof that a bridge on another chain verifies is that chain's gas, not
  this one's; here it is a client-side document. Invariant 22's rule (a
  verifier on chain needs real parameters and a gas cap) is therefore not
  triggered.
- Depend on the proof for its own safety. Full nodes verify signatures.

## Alternatives considered

- **Mina-style: every block carries a recursive proof of the whole
  chain.** Puts a prover on the block path and recursion in consensus;
  neither exists, and a prover on the block path is a liveness dependency
  on hardware nobody has measured. Rejected.
- **Signature aggregation instead of a proof (Babylon's BLS
  checkpoints).** ML-DSA does not aggregate; a PQ aggregate signature with
  a standard was not found. Rejected until one exists.
- **A SNARK with a trusted setup (Groth16 wrapper) for a small proof.**
  ADR-008 removed pairings and setups; `pqc_zk_tests.rs` checks the
  dependency graph stays that way. Rejected.
- **Adopt Plonky3-recursion now.** Unpublished, unaudited, Poseidon2-only
  by design; its own documentation declines production use. The gate in
  S2 is the written form of "not yet".
- **Do nothing; ADR-089 M0 is enough.** 152 ms per handoff at n = 100 is
  fine for a phone that is online; a bridge contract on another chain
  paying for 67 ML-DSA-87 verifications per epoch is not. P-31 is about
  the second reader.

## Consequences

- S1 adds an AIR and a prover binary (`bins/maya2c-prove-chain`, off the
  node path); `crates/light-client` gains `accept_proof(anchor, proof,
  target)`. No node change before S3.
- The proof's security is the STARK's (Plonky3 estimator, `security_bits`)
  bounded by the Keccak-256 collision bound the config states.
- Proof sizes and prover times are MISSING until measured; the ADR
  publishes none.
- The gadget work (ML-DSA-87 in-circuit) is the schedule risk and is
  named as such in the CATALOG row.

## Invariants touched

- **24**: unchanged; the proof restates what full nodes check and adds no
  apply-path rule.
- **26**: S3's `k:cmc:` record commits in the block's batch.
- **22** (retired): binds nothing here because nothing verifies on chain.
- **33**: no block-byte effect; S3 adds 32 bytes of state per epoch.
- Standing Order 4: BLAKE3 and field arithmetic; no floats, no clock.

## Activation

S1, S2: tooling, no height, no feature bit. S3:
`FeatureSet::CHAIN_PROOF_COMMITMENT = 1 << 32` (claimed in
`adr-claims.txt`; `FeatureSet` is `u64`, `crates/node/src/upgrade.rs:40`,
so bits above 31 are available), compiled
`CHAIN_PROOF_COMMITMENT_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**. Before the height
no `k:cmc:` record is written and a block that writes one has a wrong
state root (invariant 24).

## Test plan

Red tests, `crates/zk-stark/tests/handoff_chain_tests.rs`,
`crates/light-client/tests/proof_tests.rs`,
`crates/node/tests/committee_commitment_tests.rs`:

- `a_proof_over_three_handoffs_is_accepted_by_a_client_holding_the_anchor`
- `a_proof_whose_target_committee_is_not_the_records_committee_is_refused`
- `a_handoff_short_of_quorum_is_refused` (weights on a weighted chain)
- `a_proof_cannot_skip_an_epoch` (anchor epoch + K == target epoch)
- `tampering_one_header_byte_consistently_refuses` (ADR-008's negative
  test discipline, `zk-stark/src/lib.rs:20-24`)
- `security_bits_of_the_handoff_air_are_reported_not_asserted`
- `the_committee_commitment_is_written_at_the_epoch_boundary_after_the_height`
- `before_the_height_no_commitment_record_exists_and_the_root_matches_master`
- `a_reorg_restores_the_previous_commitment`

Measurements, `reports/bench/chain-proof-<date>.md`: prover time and
proof size for K = 1, 8, 64 at n = 7 and n = 100, with commit, hardware,
OS and command. The ML-DSA-87 gadget row count is reported when it
exists and written MISSING until then.

## Open questions for Eric (economics only)

None. No fee, reward or parameter changes. Who runs provers is an
operations question (a full node, an indexer); nothing pays them in this
ADR.
