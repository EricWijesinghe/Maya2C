# 5. Consensus — v0.1.0

The node runs one of two engines, chosen by its genesis: **DAG-BFT**
(Narwhal certification, Bullshark commit, `crates/node/src/consensus/bft/`)
when the genesis names a `bft` committee, and Nakamoto **proof of work**
otherwise. Under DAG-BFT a block is derived from certificates and no work is
verified, so CON-5, CON-6 and CON-7 apply to proof-of-work chains only,
CON-9 to DAG-BFT chains only, and CON-1 to CON-4 and CON-8 to both.

Node: `crates/node/src/core/block.rs`, `crates/node/src/consensus/`.

- **CON-1** A header is 145 bytes: `prev_hash[32] ‖ state_root[32] ‖ timestamp_u64 ‖ nonce_u64 ‖ difficulty_target[32] ‖ tx_root[32] ‖ version_u8`. *(positive only: defines a value)*
- **CON-2** A block's id is BLAKE3 in derive-key mode with context `"custom-l1-node header id v1"` over the 145 header bytes. *(positive only: defines a value)*
- **CON-3** `tx_root` is ROOT-3's tree over leaves `blake3_derive_key("custom-l1-node tx leaf v1", txid)`; a block whose header disagrees is invalid.
- **CON-4** A block's declared `state_root` must equal the root its execution produces (invariant 24); otherwise it is invalid and nothing is written.
- **CON-5** The declared difficulty target must equal the retarget rule's output for that height (every 100 blocks, 15 s target spacing; the 15 s is the PoW retarget spacing only, inert under DAG-BFT per CON-9, which makes about one block a second), the window measured between the median times (CON-11's median) of its last and first blocks, not between two single timestamps; otherwise invalid.
- **CON-6** The proof-of-work hash for the block's height must meet its target; otherwise invalid.
- **CON-7** The active chain is the one with the greatest cumulative work, a target's work being `2^256 ÷ (target + 1)`. *(positive only: a choice between valid branches, never a refusal)*
- **CON-8** A block at or below the prune horizon is refused.
- **CON-9** Under DAG-BFT a block's `difficulty_target` equals its parent's, so every block carries the genesis target; otherwise invalid. A DAG-BFT genesis must declare `difficulty_bits` and `pow_limit_bits` of 0 (the unlimited target). There is no retarget and no activation pin: the target certifies nothing here, and a retarget on one-second blocks drove cumulative work to 2^256 − 1 within about 12,500 blocks, which stopped the chain (ADR-035).
- **CON-10** A header's `version` is 0; any other value is invalid, and a header of another length is not a header (ADR-063).
- **CON-11** On a chain whose target is retargeted (CON-5), a block's timestamp is later than the median of the timestamps of its parent and up to ten earlier ancestors (the upper middle when fewer than eleven exist); otherwise invalid. A DAG-BFT chain (CON-9) does not apply this rule: it has no retarget to steer, and its blocks can come faster than once a second (ADR-063).
- **CON-12** A block's wire size is `145 + 8 + Σ(8 + len)` bytes, where `len` is each transaction's encoding, and it is at most the governed `block.max_bytes` in force (ParameterKey tag 21; starts at `MAX_BLOCK_BYTES` = 5 MiB (5,242,880), ADR-072, provisional; a vote may move it within [4,259,840, `BLOCK_BYTES_CEILING` = 7 MiB (7,340,032)], ADR-155). It carries at most the governed `block.max_transactions` (tag 22; starts at 65,536, within [1,024, 815,559], 815,559 = the byte ceiling at 9 bytes an entry); a decoder refuses a count above 815,559 before allocating. Both are validity parameters (CON-17). A block over the cap is invalid, whoever built it. A builder admits transactions first-fit, in order: one that would take the block over the cap is passed over and the ones after it are still considered; a transaction that does not execute costs no room, and a contract transaction is tried or passed over by the contract-work budget of GAS-9 (spec/10-vm-gas.md), which is part of this cut. The admitted indices are a function of the order, the cap, the state the block builds on and GAS-9 alone, so every node makes the same cut. *(provisional value: pending measurement on the launch host)*
- **CON-13** *(positive only: CON-13 selects payloads and never refuses one; a refusal is CON-12's `BlockTooLarge`)* A DAG-BFT builder decodes a committed sub-DAG's batch payloads within `DECODE_BUDGET` = 3/2 × the `block.max_bytes` in force (7,864,320 at the starting cap; ADR-155), shared fairly among authors by deficit round-robin. The decoded order is then cut by CON-12 and GAS-9.
  - Each author's queue is its payloads in commit order. Authors are taken in ascending `ValidatorId` (the committee index in each vertex).
  - Every turn adds `Q = max(1, DECODE_BUDGET / n)` to each author still waiting, where `n` is the committee size. The divisor is the committee size, not the number of active authors, so an honest author's first-turn credit does not depend on how many authors are active.
  - An author admits payloads from the front of its queue while the next one fits both its deficit and the budget left. A payload identical to one already admitted is skipped without a charge or a decode.
  - An author finishes when its queue is empty, or when its next payload is larger than the budget left. Its admitted set is therefore a prefix of its queue: nothing is passed over.
  - A finished author's unused deficit is dropped. The budget left keeps going to the others.
  - Admitted payloads are emitted in commit order, so execution order is the commit order restricted to the admitted set.
  - The bytes charged are raw payload bytes, per author. Payloads not admitted are not in the block and stay with their proposers.
  - Decode work per block is at most `DECODE_BUDGET`. Removing repeats compares raw bytes and decodes nothing (ADR-072).

- **CON-17** A passed governance change to a block-validity parameter (`ParameterKey::is_validity_rule`: tags 2 to 8, 17, 18, 21 and 22, the DEX fill and order ceilings, the oracle and shielded per-block caps, the VM memory, module and default-gas bounds, and the per-transaction and per-block contract gas caps of ADR-142) is not in force at its proposal's execution height `E`. It is held in `g:pending:<tag>` and is in force from height `E + MIN_UPGRADE_LEAD_BLOCKS` (2,592,000): the end-of-block pass of height `E + MIN_UPGRADE_LEAD_BLOCKS − 1` writes it into `g:params`, so the committed table at that tip is the one the activation block is judged by. At most one change waits per key; a later passed change to the same key replaces it. Tags 1, 9 and 10 (protocol fee, proposal deposit, default suite) are in force from `E + 1` as before. A change whose value is outside its key's range at `E` expires the proposal and the block is valid; a waiting value outside the range at promotion is deleted and never applied. `g:params` is written at execution only when a key in force from `E + 1` changed, and at promotion only when a value was promoted (ADR-082 amendment 2026-11-15). *(external vectors: crates/node/tests/governance_tests.rs)*

## Vectors and gaps

CON-1 to CON-3 are in `spec/tests/headers.json`, CON-4 in
`state_transitions.json` (a declared root that matches and one that does
not) and CON-5 to CON-8, CON-10, CON-11 and CON-12 in `consensus.json`. CON-6's vectors pin the
comparison of a hash with a target; the proof-of-work hash itself
(`argonblake-pow`) has no language-neutral vector.

CON-17 has no language-neutral vector: it is a rule over the governance
layer's records, pinned by `crates/node/tests/governance_tests.rs` (the
activation boundary, a replacement at `E + MIN_UPGRADE_LEAD_BLOCKS - 1`, a
mixed proposal, a dropped out-of-range value, and the record being under the
state root).

DAG construction, the commit rule, finality and fork choice under DAG-BFT
are specified only by the node's code and `crates/dag-bft`, and have no rule
IDs yet.

Catch-up by certificate replay (ADR-052) is specified by its ADR and the node,
with no rule IDs and no conformance vectors yet. Its wire rule: `get_certificates`
serves at most 256 records per page and about 8 MiB; a client refuses a larger
page, a page that makes no progress, and any certificate without a quorum
(`n − f` by the engine's rule) of valid signatures over its digest from the
epoch's committee. A page must end where its records end, and a finished epoch
is complete only if the replayed history contains its attested end (the epoch's
final checkpoint, a quorum-signed block id at a height). A peer's `done` is not
evidence of completeness. Vectors: NOT YET (gap). Node: `crates/node/src/consensus/bft/certsync.rs`.
