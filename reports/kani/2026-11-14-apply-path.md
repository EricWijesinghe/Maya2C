# Kani: apply-path harnesses (V3-P5)

Started 2026-11-14 on branch `proofs/kani-apply-path`; that attempt was cut by a
crash. Re-run in full on 2026-11-23 after merging origin/master (d6a63a5d).
Kani 0.67.0 in WSL Ubuntu-24.04 as root, tree exported with `git archive HEAD`
to `/root/kani-apply`, own `CARGO_TARGET_DIR=/root/target-kani-apply`, every run
through `cargo-slot.sh`, one harness at a time:

    cargo kani -p <pkg> --harness proofs::<name> --exact

Times are Kani's own `Verification Time` line. "No verdict" means the
`timeout` shown killed it with no `VERIFICATION` line (exit 124 or the
symbolic-execution log still growing).

## Results

| Crate | Harness | New? | Bound | Cover | Verdict | Time |
|---|---|---|---|---|---|---|
| maya-ledger-math | `a_self_transfer_moves_only_what_goes_elsewhere` | new | full 256-bit | 1 of 1 | VERIFICATION:- SUCCESSFUL | 12.5 s |
| maya-ledger-math | `a_fee_paying_transfer_conserves_value_through_the_burn` | new | full 256-bit | 1 of 1 | VERIFICATION:- SUCCESSFUL | 59.6 s |
| maya-ledger-math | `total_outputs_refuses_any_intermediate_overflow` | covers added | full 256-bit | 2 of 2 | VERIFICATION:- SUCCESSFUL | 6.1 s |
| maya-ledger-math | `a_two_output_transfer_conserves_value` | cover added | full 256-bit | 1 of 1 | VERIFICATION:- SUCCESSFUL | 4.9 s |
| maya-ledger-math | `settle_pool_agrees_with_its_inverse` | covers added | full 256-bit | 3 of 3 | VERIFICATION:- SUCCESSFUL | 7.0 s |
| maya-fee-market | `the_launch_base_fee_step_is_its_law_exactly` | new | fee, floor < 2^8; size <= `MAX_BLOCK_BYTES`; launch target and denominator | 4 of 4 | VERIFICATION:- SUCCESSFUL | 34.2 s |
| maya-staking | `bps_of_is_exact_at_the_launch_slash_rates` | new | amount < 2^16; both `Params::DEVNET` slash rates | 1 of 1 | VERIFICATION:- SUCCESSFUL | 12.7 s |
| maya-staking | `an_epoch_boundary_conserves_value` | new | amounts, counts < 2^8; one validator, one delegation | - | NO VERDICT (killed at 25 min, still in symbolic execution of BTreeMap search) | - |
| maya-staking | `bps_of_never_exceeds_the_amount` | existing | full 256-bit amount, symbolic rate | - | NO VERDICT (killed at 15 min) | - |
| maya-staking | `a_double_sign_slash_conserves_value` | existing | full 256-bit | - | NO VERDICT (killed at 15 min) | - |

7 of 10 verified, every cover statement in those 7 satisfied. No
counterexample was found, so no code changed.

## What the new harnesses say

- **Self-transfer.** `StateDB::stage_transaction` debits the whole output total
  and then credits each recipient by reading back through the overlay. With one
  output to self and one elsewhere, the sender loses exactly the other output
  and the other account gains exactly it: reading the stale balance would mint.
- **Fee through the burn.** Transfer + fee output to the collector, then
  `settle_fees` moving the owed base fee to the sink: the sender's loss equals
  recipient + collector + sink gains, the burn never fails when the fee covered
  what was owed, and the collector keeps exactly the tip.
- **Launch base-fee step.** With the divisors fixed at the launch values the
  256-bit `next_base_fee` equals the control law computed in native `u64`
  (round down, one-unit minimum rise, one-share cap, floor applied last).
  Bounded in fee magnitude; the full-width harnesses stay in `kani-heavy.yml`.
- **Launch slash rates.** `bps_of` is exactly `floor(amount * bps / 10 000)` at
  100 and 5 000 bps, not only bounded by the amount.

## Nonce

`advance_nonce_steps_by_one` (existing, full `u64`) already covers the nonce
step; no new harness.

## Where they run

`maya-staking` joins the nightly Kani matrix. Its three harnesses without a
verdict are in nightly's heavy list for the crate and in `kani-heavy.yml`
(340-minute runners), so nightly runs only the one that closes. The two
pre-existing staking harnesses had never run in CI before this change.

## Not verified

- The three staking harnesses above: no verdict, so no claim.
- The new fee-market and slash-rate harnesses are bounded proofs, stated as such
  in their doc comments.
- `cover!` satisfaction is read from the log; Kani 0.67 does not fail a harness
  on an unsatisfied cover.
