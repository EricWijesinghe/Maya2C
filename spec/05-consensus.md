# 5. Consensus — v0.1.0

The node runs one of two engines, chosen by its genesis: **DAG-BFT**
(Narwhal certification, Bullshark commit, `crates/node/src/consensus/bft/`)
when the genesis names a `bft` committee, and Nakamoto **proof of work**
otherwise. Under DAG-BFT a block is derived from certificates and no work is
verified, so CON-5, CON-6 and CON-7 apply to proof-of-work chains only,
CON-9 to DAG-BFT chains only, and CON-1 to CON-4 and CON-8 to both.

Node: `crates/node/src/core/block.rs`, `crates/node/src/consensus/`.

- **CON-1** A header is 144 bytes: `prev_hash[32] ‖ state_root[32] ‖ timestamp_u64 ‖ nonce_u64 ‖ difficulty_target[32] ‖ tx_root[32]`. *(positive only: defines a value)*
- **CON-2** A block's id is BLAKE3 in derive-key mode with context `"custom-l1-node header id v1"` over the 144 header bytes. *(positive only: defines a value)*
- **CON-3** `tx_root` is ROOT-3's tree over leaves `blake3_derive_key("custom-l1-node tx leaf v1", txid)`; a block whose header disagrees is invalid.
- **CON-4** A block's declared `state_root` must equal the root its execution produces (invariant 24); otherwise it is invalid and nothing is written.
- **CON-5** The declared difficulty target must equal the retarget rule's output for that height (every 100 blocks, 15 s target spacing); otherwise invalid.
- **CON-6** The proof-of-work hash for the block's height must meet its target; otherwise invalid.
- **CON-7** The active chain is the one with the greatest cumulative work, a target's work being `2^256 ÷ (target + 1)`. *(positive only: a choice between valid branches, never a refusal)*
- **CON-8** A block at or below the prune horizon is refused.
- **CON-9** Under DAG-BFT a block's `difficulty_target` equals its parent's, so every block carries the genesis target; otherwise invalid. A DAG-BFT genesis must declare `difficulty_bits` and `pow_limit_bits` of 0 (the unlimited target). There is no retarget and no activation pin: the target certifies nothing here, and a retarget on one-second blocks drove cumulative work to 2^256 − 1 within about 12,500 blocks, which stopped the chain (ADR-035).

## Vectors and gaps

CON-1 to CON-3 are in `spec/tests/headers.json`, CON-4 in
`state_transitions.json` (a declared root that matches and one that does
not) and CON-5 to CON-8 in `consensus.json`. CON-6's vectors pin the
comparison of a hash with a target; the proof-of-work hash itself
(`argonblake-pow`) has no language-neutral vector.

DAG construction, the commit rule, finality and fork choice under DAG-BFT
are specified only by the node's code and `crates/dag-bft`, and have no rule
IDs yet.
