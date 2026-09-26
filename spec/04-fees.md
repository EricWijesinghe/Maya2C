# 4. Fee market — v0.1.0

**Inactive on every network**: `FeeConfig::DISABLED` has activation height
`u64::MAX`. These are the rules it will enforce once a height is set.
Reference: `crates/spec-ref/src/fees.rs`. Node: `crates/fee-market`.

- **FEE-1** A base fee paid is split `treasury = ⌊base × min(bps, 10000) / 10000⌋`, `burned = base − treasury`; the tip goes whole to the producer. Nothing is created or lost. *(positive only: defines a value)* Proven for all inputs by Z3 and Lean (`formal/`).
- **FEE-2** A zero target or zero change denominator leaves the base fee at `max(parent, floor)`. *(positive only: defines a value)*
- **FEE-3** Over target the fee rises by `max(1, ⌊parent × gap / target / denom⌋)`, saturating at `u64::MAX`; under target it falls by `⌊parent × gap / target / denom⌋`, saturating at 0; at target it is unchanged. *(positive only: defines a value)*
- **FEE-4** The next base fee is never below the configured floor. *(positive only: defines a value)*
- **FEE-5** A transaction whose `max_fee` is below `base_fee × size_bytes` makes its block invalid (`Underpriced`).

## Gaps

- FEE-5 has no vector yet.
- The neural gain rule (`FeeRule::Neural`, `MODEL_V1`) is dark behind a second activation height and is specified by `crates/fee-market/src/model/` and its pinned weight digest.
