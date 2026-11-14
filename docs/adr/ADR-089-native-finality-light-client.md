# ADR-089: Native finality light client (succinct proofs, phase 0)

**Status:** Accepted for phase 0, 2026-11-10. Non-consensus. Landed as `crates/light-client` (`finality.rs`, `record.rs`); changes no node behaviour. `docs/architecture-vision.md` still tags it RESEARCH, which is the class, not this status.
**Date:** 2026-11-10
**Related:** [ADR-027](ADR-027-dag-bft-in-the-node.md) (DAG-BFT finality), [ADR-038](ADR-038-attested-checkpoints.md) (checkpoints), [ADR-040](ADR-040-stake-weighted-committee.md) (stake quorum), [ADR-046](ADR-046-universe-scale-supply.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); research note [succinct-light-client-2026-11-10](../research/succinct-light-client-2026-11-10.md); `crates/light-client/src/finality.rs`, `crates/light-client/src/record.rs`

## Context

The light client in `crates/light-client` verifies proof-of-work headers
(`lib.rs`). The chain finalises by DAG-BFT: a block is final when a quorum
of its epoch's committee attests it. A light client that only checks work
cannot follow that chain, and the research note records that no
finality-based client exists. Phase 0 of the succinct-proof plan is that
client, using no proof system.

## Decision

1. **The node's verifier is the verifier.** `FinalityClient` calls
   `Checkpoint::verify` from `custom_l1_node::consensus::bft::attest`. The
   quorum rule (n − f heads, or W − f stake on a weighted chain) is not
   reimplemented. The crate already depends on the node crate.
2. **Trust starts at an anchor.** A `TrustAnchor` is the genesis committee,
   or a checkpoint the operator already trusts. Whether the chain is
   stake-weighted is fixed by the anchor. A handoff cannot drop the weights.
3. **Committees rotate by proof, not by assertion.** An `EpochHandoff` is a
   checkpoint of epoch E, signed by the committee the client trusts, plus its
   header and staking-record proofs (`k:cmt`, `k:cmw`, `k:key`). The header
   must hash to the checkpoint block. The record leaves must sit under the
   staking layer, and the layers must fold to the header's state root.
   Each member's key must hash to its validator id.
4. **Trusted period in blocks, shorter than unbonding.** Heights are the
   only clock. `TrustPolicy::new` refuses a window that is zero or at least
   the unbonding period, so a validator that has left cannot sign a fork a
   client still trusts. Checkpoints below the trusted floor, or below one
   already accepted, are refused.
5. **Refusals change nothing.** Every error leaves the client as it was.

## Consequences

- The staking layer is one BLAKE3 Merkle tree over its records
  (`state/db.rs`, `record_leaf`). A record proof therefore carries the
  Merkle path and every layer digest. This is larger than an account proof
  and is what the state offers today. Sparse records would need a node
  change, which is post-genesis.
- `record_leaf` is ported, because the node keeps it `pub(crate)`. The test
  `the_ported_leaf_reproduces_the_node_staking_root` pins the port to the
  node's own staking-layer root.
- No node-side prover exists yet. Handoff proofs are built in tests from the
  node's public `merkle_path` and `account_proof`. Serving them over RPC is
  not built.
- Which block first commits `k:cmt:<E+1>`, and which committee attests it, is
  not settled here. A handoff that finds no such checkpoint fails closed.
- Weak subjectivity remains. The trusted period is an assumption and not a
  proof.

## Not decided

Consensus-visible committee commitments (research note, phase 3), STARK
aggregation (phase 2), and any change to the frozen node binary.
