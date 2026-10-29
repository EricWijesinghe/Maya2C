# 2026-10-26: audit of the areas the freeze audit never covered

Two read-only agents (blockchain-security-auditor, security-architect) read the
code; nothing below was run by them. "Confirmed" means a test in this session
failed before the fix or fails now, with the command output seen.

## Fixed this session (test first, where marked)

| Finding | What | Evidence |
|---|---|---|
| L-1 identity revocation index | `index / 4096 as u32` wrapped; `1<<44` revoked credential 0. Now refused on write, reads as revoked. | `identity_tests::a_revocation_index_past_the_last_page_is_refused_not_wrapped` failed, then 16/16 pass |
| B (audit 4, L-2) vault window leak | A failed vault transfer left its spend in the selection overlay and starved the valid transfer behind it. | `vault_tests::a_failed_vault_transfer_does_not_spend_the_window_for_the_next_one` failed, then 7/7 pass |
| C (audit 3) storage errors swallowed in selection | `select_applicable` returned an empty block on a read error and dropped transfers on storage errors. Now `Err`. | compiled; **no test** (no storage-fault injection point) |
| D (audit 5) preview key ignored multisig approvals | key hashed txids, which omit approvals; now hashes wire bytes. | `preview::tests::two_quorums_of_one_multisig_spend_do_not_share_a_preview` passes; not run against the old key |
| L-3 DEX escrow `saturating_sub` | now `checked_sub` -> `InvariantViolation`. | compiled; **no test** (audit argued it is unreachable) |
| M-1 `HybridSigningKey` zeroize | **sound**: both upstream secret-key types implement `ZeroizeOnDrop`; a compile-time bound now pins it in `keys.rs` and `sig.rs`. | both tests pass |
| #8 `import_headers` skipped refusals | now applies `refuse_unsupported` and the prune-horizon refusal. | `pruned_node_tests::a_pruned_node_refuses_imported_headers_at_or_below_its_horizon`, 18/18 pass |

All of the "fixed" rows change what a node accepts or selects, so they take
effect only at the next re-genesis.

## Confirmed, needs Eric's decision (not fixed)

- **H-1 DEX dust halts a pair.** A resting ask at price 1, amount 1 is the best
  ask and the oldest order; the cross yields a zero quote and `match_book`
  stops the whole pass. Test
  `matching_tests::a_resting_dust_order_does_not_freeze_the_honest_orders_behind_it`
  fails today and is `#[ignore]`d. Options: (a) cancel and refund the order
  that would be fully consumed by a zero-quote fill and continue; (b) reject
  placements below a minimum notional (does not cover partial-fill dust);
  (c) both. I recommend (a)+(b). Changes matching, needs spec, vectors, ADR.
  Not checked: that the on-chain placement path accepts price 1 / amount 1
  (the audit read `dex_exec.rs:804-810`; I did not run it).
- **H-2 governance quorum denominator.** Vote weight = locked stake + work
  credit; `eligible` = locked stake only (`governance_exec.rs:451`, `:512`), and
  work-only voters skip the hold-through-execution rule. With a small locked
  total, work credit alone carries quorum. Read from code, not tested.
  Policy: include work in `eligible`, or drop it from turnout, and decide the
  hold rule for work weight.
- **Timestamp steering of retarget (audit 2).** Unbounded header timestamps feed
  the retarget span (clamped x4 per window). ADR-063's drift bound does not
  close it and reads a clock. Needs the ADR-063 decision.
- **Retarget window off by one (audit 7).** 99 gaps walked, 100 assumed;
  about 1% drift per window at exactly 15 s spacing. Consensus change.

## Not fixed, with reason

- **PoW verified while the chain mutex is held (audit 1).** `submit_block` takes
  the lock and then runs Argon2id. Applies only where `submit_block` is enabled;
  it is refused on DAG-BFT networks (`server.rs:794`). Fix needs a
  verify-before-lock split in `Chain`; not done.
- **Breaker trip can shorten a council pause (audit 6)**, **`adopt_heaviest`
  retries a failing branch every boot (audit 9)**, **pruned node cannot leave a
  minority chain (audit 10, by design)**: latent or documented; no change.
- **L-4 encode-side `len() as u8`** in identity/RWA payload encoders: not checked
  whether constructors bound the lengths.

## Not reached by either auditor

Certificate/checkpoint signing, `settle_*` error semantics, `conservation.rs`
body, archive read-back, `hashimoto_light`, `dex/batch.rs` and `amm.rs` swap
math, shielded frontier decode, snapshot decoders in detail, reorg in DAG-BFT
mode, oracle beacon/VRF, BFT `wire.rs`, rwa predicates, mempool admission against
breakers. These remain unaudited.
