# 4. Fee market — v0.1.1

Active on a chain whose genesis carries `bft.fees`; every committee genesis
`genesis-ceremony` writes carries it (`LAUNCH_FEES`). The library's own
research configuration, `FeeConfig::DISABLED`, keeps activation height
`u64::MAX`. Reference: `crates/spec-ref/src/fees.rs`. Node:
`crates/node/src/state/fees.rs` and `fee_exec.rs`, over `crates/fee-market`.

v0.1.1 (2026-11-16): FEE-1 states the burn the node applies. v0.1.0 described
the treasury split as live; no node has ever paid a treasury share.

- **FEE-1** The whole base fee paid burns: at the end of its block it moves from the fee collector to the unspendable `FEE_SINK`, so `total` supply is conserved and `circulating` falls. The tip stays in the fee collector. On a chain with staking (`bft.staking`) the whole collector becomes the staking reward pool at the next epoch boundary; on a chain without staking the tip stays in the fee collector. Nothing is created or lost. The split function is `treasury = ⌊base × min(bps, 10000) / 10000⌋`, `burned = base − treasury`, and every node genesis fixes `bps = 0` (`FeesGenesis::record`), so `treasury = 0` and `burned = base` (ADR-029, the recorded 100% base-fee burn). Vectors at other `bps` pin the split function, not a network parameter. *(positive only: defines a value)* The split is proven for all inputs by Z3 and Lean (`formal/`); the burn is pinned by `crates/node/tests/fee_market_live_tests.rs` `an_unpaid_transfer_is_refused_and_a_paid_one_burns_its_base_fee`.
- **FEE-2** A zero target or zero change denominator leaves the base fee at `max(parent, floor)`. *(positive only: defines a value)*
- **FEE-3** Over target the fee rises by `min(max(1, ⌊parent × gap / target / denom⌋), max(1, ⌊parent / denom⌋))` (one denominator's share per block, ADR-076); under target it falls by `⌊parent × gap / target / denom⌋`, saturating at 0; at target it is unchanged. *(positive only: defines a value)*
- **FEE-4** The next base fee is never below the configured floor. *(positive only: defines a value)*
- **FEE-5** A transaction whose `max_fee` is below `base_fee × size_bytes` makes its block invalid (`Underpriced`).
- **FEE-6** A rise ends at `max(MAX_BASE_FEE_PER_BYTE, parent)`: no block raises the base fee above the compiled-in ceiling, and a fee already above it holds. The ceiling is a placeholder pending the economics decision in ADR-076. *(positive only: defines a value)*
- **FEE-7** A mining share (`TxKind::MiningShare`, ADR-044) is exempt from the base fee only when it carries no inputs and no outputs. When the chain charges fees, a share that carries either owes the base fee on its size like any other transaction, and a block carrying one whose fee-collector outputs fall short is invalid (`FeeTooLow`); the mempool refuses it. The exemption does not look at the authorization: a share's `multisig` or `suite_auth` bytes are bounded by `MAX_SIGNERS`, the block byte cap and the per-block share cap, not by the fee. Node: `crates/node/src/state/fee_exec.rs` `is_free_share` and `required_fee`, shared by the executor and the mempool. Vectors replay the predicate and the requirement; the block-invalid path through `charge_fee` is pinned by `crates/node/tests/mining_rewards_tests.rs`.

## Gaps

- FEE-5 has no vector yet.
- FEE-1's routing is a state-level rule that the vectors do not reach: the base fee moving from the collector to `FEE_SINK` at the end of each block is pinned by `crates/node/tests/fee_market_live_tests.rs`, and the collector becoming the staking reward pool at the epoch boundary (`fee_exec.rs` `take_fee_pool`, called from `staking_exec.rs` `end_epoch`) is not yet pinned by any test. The vectors cover the split function only.
- No neural gain rule is part of this spec. The RESEARCH model (`crates/neural-fee`) is not linked by the node (ADR-029 amendment 2026-11-14); adopting it would be a hard fork with its own section and vectors.
