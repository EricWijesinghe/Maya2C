# 4. Fee market — v0.1.1

Active on a chain whose genesis carries `bft.fees`; every committee genesis
`genesis-ceremony` writes carries it (`LAUNCH_FEES`). The library's own
research configuration, `FeeConfig::DISABLED`, keeps activation height
`u64::MAX`. Reference: `crates/spec-ref/src/fees.rs`. Node:
`crates/node/src/state/fees.rs` and `fee_exec.rs`, over `crates/fee-market`.

v0.1.1 (2026-11-16): FEE-1 states the burn the node applies. v0.1.0 described
the treasury split as live; no node has ever paid a treasury share.

- **FEE-1** The whole base fee paid burns: at the end of its block it moves from the fee collector to the unspendable `FEE_SINK`, so `total` supply is conserved and `circulating` falls. The tip stays in the fee collector and becomes the staking reward pool at the next epoch boundary. Nothing is created or lost. The split function is `treasury = ⌊base × min(bps, 10000) / 10000⌋`, `burned = base − treasury`, and every node genesis fixes `bps = 0` (`FeesGenesis::record`), so `treasury = 0` and `burned = base` (ADR-029, the recorded 100% base-fee burn). Vectors at other `bps` pin the split function, not a network parameter. *(positive only: defines a value)* The split is proven for all inputs by Z3 and Lean (`formal/`); the burn is pinned by `crates/node/tests/fee_market_live_tests.rs` `an_unpaid_transfer_is_refused_and_a_paid_one_burns_its_base_fee`.
- **FEE-2** A zero target or zero change denominator leaves the base fee at `max(parent, floor)`. *(positive only: defines a value)*
- **FEE-3** Over target the fee rises by `min(max(1, ⌊parent × gap / target / denom⌋), max(1, ⌊parent / denom⌋))` (one denominator's share per block, ADR-076); under target it falls by `⌊parent × gap / target / denom⌋`, saturating at 0; at target it is unchanged. *(positive only: defines a value)*
- **FEE-4** The next base fee is never below the configured floor. *(positive only: defines a value)*
- **FEE-5** A transaction whose `max_fee` is below `base_fee × size_bytes` makes its block invalid (`Underpriced`).
- **FEE-6** A rise ends at `max(MAX_BASE_FEE_PER_BYTE, parent)`: no block raises the base fee above the compiled-in ceiling, and a fee already above it holds. The ceiling is a placeholder pending the economics decision in ADR-076. *(positive only: defines a value)*

## Gaps

- FEE-5 has no vector yet.
- The neural gain rule (`FeeRule::Neural`, `MODEL_V1`) is dark behind a second activation height and is specified by `crates/fee-market/src/model/` and its pinned weight digest.
