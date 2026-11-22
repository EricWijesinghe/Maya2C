# Kani: arithmetic of the apply-path order (V3-P5)

**Scope, stated first.** These harnesses prove the arithmetic the apply path
is built from, and this crate's model of the order it applies it in. They do
not call the node's own apply functions — `StateDB::stage_transaction`
(`crates/node/src/state/db.rs`), `settle_fees` (`crates/node/src/state/fee_exec.rs`)
or the node's `end_epoch` (`crates/node/src/state/staking_exec.rs`) — and
`crates/node` has no Kani harness. The node's own apply functions are not
proven here; ordinary tests pin them (for the self-transfer,
`self_transfer_nets_to_zero_and_does_not_mint` in
`crates/node/tests/state_tests.rs`). Only the staking epoch harness calls the
real `maya-staking` `end_epoch`, and it has no verdict.

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
| maya-fee-market | `the_launch_base_fee_step_is_its_law_exactly` | new | fee, floor < 2^8; size <= `MAX_BLOCK_BYTES`; target `MAX_TARGET_BLOCK_BYTES`, denominator 8 | 4 of 5 (the cap cover UNSATISFIABLE: the cap cannot bind here) | VERIFICATION:- SUCCESSFUL | 15.4 s |
| maya-fee-market | `the_base_fee_step_is_its_law_where_the_cap_binds` | new (2026-11-23 review) | as above, target 1 MiB (`FeeConfig::TESTING`) | 5 of 5 | VERIFICATION:- SUCCESSFUL | 16.8 s |
| maya-staking | `bps_of_is_exact_at_the_launch_slash_rates` | new | amount < 2^16; both `Params::DEVNET` slash rates | 1 of 1 | VERIFICATION:- SUCCESSFUL | 15.4 s (2026-11-14 log) |
| maya-staking | `an_epoch_boundary_conserves_value` | new | amounts, counts < 2^8; one validator, one delegation | - | NO VERDICT (killed at 25 min, still in symbolic execution of BTreeMap search) | - |
| maya-staking | `bps_of_never_exceeds_the_amount` | existing | full 256-bit amount, symbolic rate | - | NO VERDICT (killed at 15 min) | - |
| maya-staking | `a_double_sign_slash_conserves_value` | existing | full 256-bit | - | NO VERDICT (killed at 15 min) | - |

8 of 11 verified. Every cover in those 8 is satisfied except the one-share-cap
cover at the `MAX_TARGET_BLOCK_BYTES` target, which Kani reports UNSATISFIABLE
— the expected result, since there the gap is at most the target and the cap
cannot bind. No counterexample was found, so no code outside `proofs.rs`
changed.

Tried on 2026-11-23 and dropped, no verdict: a double-sign slash with bond and
delegation below 2^16 on the real state machine, and a down epoch (counts and
rates concrete, stake below 2^16). Both hit `timeout 900` (exit 124) still
unwinding `memcmp` on the 32-byte `BTreeMap` keys, so the obstacle is the
collections, not the arithmetic width. Neither harness was kept.

## Raw lines

Verbatim from the logs (`grep -E "^Checking harness|VERIFICATION:|cover properties satisfied|Verification Time"`).
2026-11-23 runs from `/root/kani-apply-lm-*.log`, `/root/kani-apply-fee.log`,
and (after the review) `/root/kani-apply-58-logs/`; the slash-rate run from
`/root/kani-apply-logs/` (2026-11-14). The table's earlier 12.7 s for the
slash-rate harness had no log behind it; 15.43 s is the logged figure.

    Checking harness proofs::a_self_transfer_moves_only_what_goes_elsewhere...
     ** 1 of 1 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 12.476292s
    Checking harness proofs::a_fee_paying_transfer_conserves_value_through_the_burn...
     ** 1 of 1 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 59.637405s
    Checking harness proofs::total_outputs_refuses_any_intermediate_overflow...
     ** 2 of 2 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 6.101902s
    Checking harness proofs::a_two_output_transfer_conserves_value...
     ** 1 of 1 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 4.880266s
    Checking harness proofs::settle_pool_agrees_with_its_inverse...
     ** 3 of 3 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 7.0271454s
    Checking harness proofs::the_base_fee_step_is_its_law_where_the_cap_binds...
     ** 5 of 5 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 16.821377s
    Checking harness proofs::the_launch_base_fee_step_is_its_law_exactly...
     ** 4 of 5 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 15.352043s
    Check 33: proofs::check_the_base_fee_law_at.cover.5
    	 - Status: UNSATISFIABLE
    	 - Description: "the one-share cap binds"
    Checking harness proofs::bps_of_is_exact_at_the_launch_slash_rates...
     ** 1 of 1 cover properties satisfied
    VERIFICATION:- SUCCESSFUL
    Verification Time: 15.42812s
    == maya-staking a_bounded_double_sign_slash_conserves_value exit=124
    Checking harness proofs::a_bounded_double_sign_slash_conserves_value...
    == maya-staking a_down_epoch_slashes_and_conserves_value exit=124
    Checking harness proofs::a_down_epoch_slashes_and_conserves_value...

The three staking rows marked NO VERDICT have only their `Checking harness`
line in their logs (`/root/kani-apply-epoch.log`,
`/root/kani-apply-st-*.log`): no `VERIFICATION` line.

## What the new harnesses say

- **Self-transfer (model).** `StateDB::stage_transaction` debits the whole
  output total and then credits each recipient by reading back through the
  overlay. The harness proves `ledger-math`'s model of that order (built from
  `total_outputs`, `debit`, `credit`): with one output to self and one
  elsewhere, the sender loses exactly the other output and the other account
  gains exactly it. It does not guard the node, which calls `U256`
  arithmetic directly; `self_transfer_nets_to_zero_and_does_not_mint`
  (`crates/node/tests/state_tests.rs`) pins `stage_transaction` itself.
- **Fee through the burn (model).** Transfer + fee output to the collector,
  then the owed base fee moved to the sink, in the order `settle_fees` does
  it: the sender's loss equals recipient + collector + sink gains, the burn
  never fails when the fee covered what was owed, and the collector keeps
  exactly the tip. A proof of the model, not of `settle_fees`.
- **Base-fee step (re-statement check).** With the divisors fixed, the 256-bit
  `next_base_fee` equals the control law re-written in native `u64` (round
  down, one-unit minimum rise, floor applied last). It proves the code matches
  a second write of the same rule, not that the rule is right. At the
  `MAX_TARGET_BLOCK_BYTES` target (2,621,440, the public testnet genesis
  value; the mainnet target is set at genesis and is not confirmed here) the
  one-share cap and `MAX_BASE_FEE_PER_BYTE` cannot bind, so that harness does
  not pin them. The 1 MiB-target harness reaches the cap (cover satisfied) and
  pins it. `MAX_BASE_FEE_PER_BYTE` binds in neither (fees below 2^8). Bounded
  in fee magnitude; the full-width harnesses stay in `kani-heavy.yml`.
- **Launch slash rates.** `bps_of` is exactly `floor(amount * bps / 10 000)` at
  100 and 5 000 bps, not only bounded by the amount, for amounts below 2^16.

## Nonce

`advance_nonce_steps_by_one` (existing, full `u64`) already covers the nonce
step; no new harness.

## Where they run

`maya-staking` joins the nightly Kani matrix. Its three harnesses without a
verdict are in nightly's heavy list for the crate and in `kani-heavy.yml`
(340-minute runners), so nightly runs only the one that closes. The two
pre-existing staking harnesses had never run in CI before this change.

## Not verified

- **Staking reward and slash conservation is an open item.** V3-P5 asked for
  it; no reward or slash conservation harness has a verdict. The only staking
  verdict is `bps_of_is_exact_at_the_launch_slash_rates` (amount < 2^16). The
  three staking harnesses above: no verdict, so no claim. Next step: a harness
  whose collections are not `BTreeMap`s keyed by 32-byte ids (or a Kani stub
  for the key comparison), so CBMC can finish symbolic execution.
- The epoch harness's start state is built by hand; its doc comment says why
  it is reachable.
- `cargo xtask proof-ratchet` was not run: the xtask does not compile on this
  branch (`xtask/src/devlog.rs` uses `chrono`; `xtask/Cargo.toml` has no
  `chrono` dependency; fixed by #649, not yet on master). Re-tried after the
  review through the lock: `cargo xtask proof-ratchet --check` ended with
  `error: could not compile \`xtask\` (bin "xtask") due to 3 previous errors`
  (E0433 on `chrono::` in `devlog.rs:84`). The baseline
  `kani_harnesses = 96` is hand-set from an anchored count of
  `#[kani::proof]` lines (`rg -c "^\s*#\[kani::proof\]"` summed = 96). The
  ratchet counts harnesses, not verdicts: three of the 96 have none. Next
  step: `cargo xtask proof-ratchet --check` once #649 lands.
- The node's own apply functions are not proven (see Scope).
- The new fee-market and slash-rate harnesses are bounded proofs, stated as such
  in their doc comments.
- `cover!` satisfaction is read from the log; Kani 0.67 does not fail a harness
  on an unsatisfied cover.
