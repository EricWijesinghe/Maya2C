# 5. Consensus — v0.1.0

What the node enforces **today** is Nakamoto proof of work. The mainnet
target, DAG-BFT (Narwhal certification, Bullshark commit), exists as
`crates/dag-bft` with simulation tests and is **not wired into the node**
(ADR-015); its rules are listed here as gaps, not specified as if they ran.

Node: `crates/node/src/core/block.rs`, `crates/node/src/consensus/`.

- **CON-1** A header is 144 bytes: `prev_hash[32] ‖ state_root[32] ‖ timestamp_u64 ‖ nonce_u64 ‖ difficulty_target[32] ‖ tx_root[32]`. *(positive only: defines a value)*
- **CON-2** A block's id is BLAKE3 in derive-key mode with context `"custom-l1-node header id v1"` over the 144 header bytes. *(positive only: defines a value)*
- **CON-3** `tx_root` is ROOT-3's tree over leaves `blake3_derive_key("custom-l1-node tx leaf v1", txid)`; a block whose header disagrees is invalid.
- **CON-4** A block's declared `state_root` must equal the root its execution produces (invariant 24); otherwise it is invalid and nothing is written.
- **CON-5** The declared difficulty target must equal the retarget rule's output for that height (every 100 blocks, 15 s target spacing); otherwise invalid.
- **CON-6** The proof-of-work hash for the block's height must meet its target; otherwise invalid.
- **CON-7** The active chain is the one with the greatest cumulative work.
- **CON-8** A block at or below the prune horizon is refused.

## Gaps (no vectors)

CON-1 … CON-8 are pinned by the node's own tests (`chain.rs`, `tx_root`,
`crash_consistency_tests.rs`) but have no language-neutral vectors yet.
DAG construction, the commit rule, finality and fork-choice under DAG-BFT
are specified only by `crates/dag-bft` and have no rule IDs until it is
wired in.
