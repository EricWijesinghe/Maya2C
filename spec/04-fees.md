# 4. Fee market — v0.2.0

**Live wherever genesis configures fees** (`bft.fees`, ADR-029): the node
keeps the record `k:fee` from block 1 and runs FEE-1..FEE-7 on every block.
A genesis without `bft.fees` has no record and charges nothing. Reference:
`crates/spec-ref/src/fees.rs`. Node: `crates/node/src/state/fee_exec.rs` and
`state/fees.rs`, which take the step rule from `crates/fee-market`.
(`FeeConfig::DISABLED` in `crates/fee-market` configures that crate's own
standalone executor, which the node does not run.)

The node's split is FEE-1 with `bps = 0` on every network: the whole base fee
burns and nothing goes to a treasury (FEE-7). This is a guarantee, not a
default somebody forgot: no key holder, council or vote can redirect fees,
because no record field or parameter names a treasury share. Adding one is a
scheduled protocol upgrade with its own ADR, never a runtime switch.

- **FEE-1** A base fee paid is split `treasury = ⌊base × min(bps, 10000) / 10000⌋`, `burned = base − treasury`. Nothing is created or lost. The node runs it with `bps = 0` (FEE-7), so `burned = base`; the tip is not part of the split (FEE-7 says where it goes). *(positive only: defines a value)* Proven for all inputs by Z3 and Lean (`formal/`).
- **FEE-2** A zero target or zero change denominator leaves the base fee at `max(parent, floor)`. *(positive only: defines a value)*
- **FEE-3** Over target the fee rises by `min(max(1, ⌊parent × gap / target / denom⌋), max(1, ⌊parent / denom⌋))` (one denominator's share per block, ADR-076); under target it falls by `⌊parent × gap / target / denom⌋`, saturating at 0; at target it is unchanged. *(positive only: defines a value)*
- **FEE-4** The next base fee is never below the configured floor. *(positive only: defines a value)*
- **FEE-5** A transaction pays its fee as signed outputs to `FEE_COLLECTOR`; there is no fee field. One whose collector outputs total less than `base_fee × size_bytes`, where `size_bytes` is its serialized length, makes its block invalid (`FeeTooLow`). A mining share that moves nothing owes 0 (ADR-044); its bytes still count toward the step (FEE-3). *(external vectors: crates/node/tests/fee_market_live_tests.rs)*
- **FEE-6** A rise ends at `max(MAX_BASE_FEE_PER_BYTE, parent)`: no block raises the base fee above the compiled-in ceiling, and a fee already above it holds. The ceiling is a placeholder pending the economics decision in ADR-076. *(positive only: defines a value)*
- **FEE-7** At the end of each block the node moves exactly the base fees its transactions owed, `Σ base_fee × size_bytes`, from `FEE_COLLECTOR` to `FEE_SINK` (burned: total supply is conserved, circulating supply falls). What a transaction paid the collector above its base fee is its tip; tips stay in the collector until the staking epoch ends, when the whole collector is the epoch's reward pool, shared by the staking rules (ADR-028), not given to the block's producer. A collector holding less than the block's burn makes the block invalid (`InvariantViolation`). *(external vectors: crates/node/tests/fee_market_live_tests.rs)*

## Gaps

- FEE-5 and FEE-7 have no vector in `spec/tests/`; `fee_market_live_tests::an_unpaid_transfer_is_refused_and_a_paid_one_burns_its_base_fee` checks both on the node (refused unpaid, exact burn, tip kept for the epoch).
- v0.1.0 described `crates/fee-market`'s standalone executor (a `max_fee` field, tips to the producer) as if it were the node's rule. v0.2.0 states what the node does (Eric 2026-11-24, FEE-1).
- The neural gain rule (`FeeRule::Neural`, `MODEL_V1`) is dark behind a second activation height and is specified by `crates/fee-market/src/model/` and its pinned weight digest.
