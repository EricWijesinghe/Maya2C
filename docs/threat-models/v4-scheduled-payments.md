# Threat model: scheduled payments (v4, P-7)

ADR: [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-7 (design text in ADR-109, PR #465; `TopUpSchedule` cut).
Written 2026-11-12. Tests: pending (build job I6).

## Attacker

A payer (wants to stop paying without cancelling, or to make the payee's
run fail), a payee (wants more than the schedule), a griefer with coin who
wants every block to do maximal work, a block producer (wants to omit or
reorder due runs), and ADR-114's timers sharing the end-of-block step.

## Assets

- Escrow balances (`sp:s:` escrow): must equal what was debited minus what
  ran.
- Block liveness: the end-of-block step's cost must be bounded no matter
  how many schedules exist.
- Determinism of the step (invariant 24): every node computes the same
  runs in the same order.
- Other users' schedules (fair carry-over).

## Attacks

| # | Attack | Path |
|---|---|---|
| S-1 | **Block stuffing**: 100,000 schedules all due at height H; every block from H does 256 runs forever. | Bounded at 256 per block; each run was prepaid (`RUN_FEE` + amount); the attacker locks `100,000 × (amount + RUN_FEE)` to do it. Transactions are never delayed, schedules are. |
| S-2 | **Starvation**: the attacker's 100,000 schedules carry over and always have the lowest `next_due`, so an honest schedule due at H+1 waits 390 blocks. | Accepted and stated as the bound: carry-over is by `next_due` then id, so the honest schedule runs after the backlog. `RUN_FEE` (O-6) is the deterrent; a per-payer cap is **not** added (it would be a map keyed by payer under consensus). |
| S-3 | **Escrow underflow**: `escrow - (amount + RUN_FEE)` wraps when `escrow` is short. | `>=` check first; `checked_sub`. |
| S-4 | **Escrow overflow at creation**: `(amount + RUN_FEE) × runs` wraps so a tiny debit funds a huge schedule. | `checked_add` then `checked_mul`; bound `< 2^249` written; vector at the bound. |
| S-5 | **Producer omits a due run** and declares a root without it. | Invariant 24: the root must reproduce the step; the block is refused. |
| S-6 | **Producer runs a schedule early** (height H-1). | `next_due <= height` strict; vector. |
| S-7 | **Cancel race**: payer cancels in the same block the run is due; order decides who gets the amount. | Transactions apply before the step: a cancel in block H wins over a run due at H. Stated; vector. |
| S-8 | **Miss then refund**: payer lets three runs miss by never funding... but escrow was prepaid, so a miss can only occur if... | A miss occurs only when `escrow < amount + RUN_FEE`, i.e. the escrow was sized for `runs` at creation and cannot be short unless `RUN_FEE` rose by governance (invariant 17). The `missed` path exists for that case. |
| S-9 | **Governance raises `RUN_FEE` above escrow** to strand every schedule. | Each miss is a no-op; the third closes the schedule and refunds the payer; nothing is lost. |
| S-10 | **Reorg**: a run applied at H is undone; the escrow and `sp:d:` row must come back. | Overlay records under the undo journal; the step writes through the same overlay. |
| S-11 | **Breaker**: `Module::Scheduled` tripped; due rows grow; on resume the backlog runs 256 per block. | Accepted (S-8 in the ADR's pass). Prepaid, in order. |
| S-12 | **Timer interleave**: ADR-114's timers run in the same step and a timer's contract reads a balance a schedule is about to credit. | Fixed order: schedules then timers; stated in both ADRs; vector when ADR-114 lands. |
| S-13 | **Vault escape**: a vault creates a schedule to move its whole balance out as escrow in one transaction. | The escrow debit is a vault-limited movement (invariant 32). |
| S-14 | **Payee is the fee collector or the zero address**. | `payee` must not be `FEE_COLLECTOR`, `FEE_SINK` or `[0;32]`; refused at creation. |

## Mitigations

As the table. The one accepted residual is S-2 (starvation behind a prepaid
backlog); its price is `RUN_FEE × runs` per attacker schedule and it never
touches transactions.

## Replay tests (`crates/node/tests/scheduled_payment_tests.rs`)

1. `a_schedule_runs_at_its_due_height_and_not_one_block_earlier`
2. `the_257th_due_schedule_runs_first_in_the_next_block`
3. `one_thousand_schedules_due_at_one_height_delay_schedules_not_transactions` (chaos_simulator scenario)
4. `escrow_arithmetic_at_the_2_pow_249_bound_is_accepted_and_one_over_refused`
5. `a_block_whose_root_omits_a_due_run_is_refused`
6. `a_cancel_in_the_due_block_wins_over_the_run`
7. `a_run_fee_raised_above_escrow_misses_three_times_then_refunds`
8. `a_reorg_restores_escrow_next_due_and_the_due_index`
9. `a_scheduled_breaker_skips_the_step_and_resumes_in_order`
10. `a_vault_counts_the_escrow_debit_once_and_runs_not_at_all`
11. `a_payee_of_fee_collector_fee_sink_or_zero_is_refused`
12. `the_root_is_unchanged_when_no_schedule_is_due`
13. Worst-case block timing (plan I6 "worst-case block test"): 256 runs in
    one block, measured wall time with the command and hardware recorded in
    `reports/` — a number, not a target.
