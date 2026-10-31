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
- **CON-5** The declared difficulty target must equal the retarget rule's output for that height (every 100 blocks, 15 s target spacing), the window measured between the median times (CON-11's median) of its last and first blocks, not between two single timestamps; otherwise invalid.
- **CON-6** The proof-of-work hash for the block's height must meet its target; otherwise invalid.
- **CON-7** The active chain is the one with the greatest cumulative work, a target's work being `2^256 ÷ (target + 1)`. *(positive only: a choice between valid branches, never a refusal)*
- **CON-8** A block at or below the prune horizon is refused.
- **CON-9** Under DAG-BFT a block's `difficulty_target` equals its parent's, so every block carries the genesis target; otherwise invalid. A DAG-BFT genesis must declare `difficulty_bits` and `pow_limit_bits` of 0 (the unlimited target). There is no retarget and no activation pin: the target certifies nothing here, and a retarget on one-second blocks drove cumulative work to 2^256 − 1 within about 12,500 blocks, which stopped the chain (ADR-035).
- **CON-10** A header's `version` is 0; any other value is invalid, and a header of another length is not a header (ADR-063).
- **CON-11** On a chain whose target is retargeted (CON-5), a block's timestamp is later than the median of the timestamps of its parent and up to ten earlier ancestors (the upper middle when fewer than eleven exist); otherwise invalid. A DAG-BFT chain (CON-9) does not apply this rule: it has no retarget to steer, and its blocks can come faster than once a second (ADR-063).
- **CON-12** A block's wire size is `145 + 8 + Σ(8 + len)` bytes, where `len` is each transaction's encoding, and it is at most `MAX_BLOCK_BYTES` = 5 MiB (5,242,880) (ADR-072, provisional). A block over the cap is invalid, whoever built it. A builder admits transactions in order and stops at the first that would take the block over the cap; the rest stay out of the block, so the cut is a prefix and every node makes the same one. *(provisional value: pending measurement on the launch host)*

## Vectors and gaps

CON-1 to CON-3 are in `spec/tests/headers.json`, CON-4 in
`state_transitions.json` (a declared root that matches and one that does
not) and CON-5 to CON-8, CON-10, CON-11 and CON-12 in `consensus.json`. CON-6's vectors pin the
comparison of a hash with a target; the proof-of-work hash itself
(`argonblake-pow`) has no language-neutral vector.

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
