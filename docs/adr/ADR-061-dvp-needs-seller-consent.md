# ADR-061: RWA delivery-versus-payment is off until the seller consents

Status: Accepted (2026-10-26). Consensus change, pre-genesis: no live chain runs the swap.

## Context
The freeze-readiness audit (2026-10-26) found that `SettleDvp` takes the seller
from the payload and only the buyer signs (`state/rwa_exec.rs` `settle_dvp`,
`state/settlement.rs`). Nothing asks the seller. Any account could submit
`SettleDvp{seller: holder, units: all, price: 0}` for an unrestricted asset and
take the holder's units for one fee. A swap with the sender as both parties
credited the price with no debit; the conservation guard refused the block, so
one fee could empty a block.

## Decision
1. `SettleDvp` executes only from `DVP_ACTIVATION_HEIGHT`, which is `u64::MAX`:
   the research-branch pattern every unfinished subsystem uses. Before it, the
   transaction is a no-op, not an error, so a swap that cannot run cannot void a
   block (invariant 7).
2. A swap with buyer equal to seller is a no-op (`DvpOutcome::SelfTrade`).
3. The activation height is written only after the swap carries the seller's
   consent: a seller-signed offer naming asset, units, price, buyer, page and a
   single-use identifier, consumed by the swap so it cannot replay. That is a
   wire change, and the offer's signature size must be counted against
   `MAX_TX_BYTES`.

## Consequences
Tests: `swaps_are_off_until_the_seller_can_consent` fails without the gate (the
exploit moved the issuer's units); `a_swap_with_oneself_moves_nothing`. The
other RWA tests run with the gate opened by `with_dvp_activation(0)`. The
`features.toml` entry drops from `working` to `stub`. Issuance, distribution
and eligibility are unchanged.
