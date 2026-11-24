# Threat model: sponsored fees (v4, P-4, wire v13)

ADR: [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-4 (design text in ADR-106, PR #465; version byte 0x0D).
Written 2026-11-12. Tests: pending (build job I4).

## Attacker

A sponsored user (wants more than one fee out of one signature), a relay
(can add, strip or swap outer frames), a block producer, a rival sponsor,
and the sponsor itself (wants to make its users' transactions fail without
cost).

## Assets

- The sponsor's native balance (the drain target).
- The sender's nonce (must never be burned by a sponsor's failure).
- Mempool capacity (frames whose validity depends on a third party's
  balance).
- The base-fee burn and tip split (must not change for sponsored frames).

## Attacks

| # | Attack | Path |
|---|---|---|
| P-1 | **Drain by reuse**: one sponsorship applied to many transactions. | Signed over `txid(inner)`, which covers the nonce. |
| P-2 | **Drain by ceiling**: a sponsor signs `max_fee = X`; the user pads the inner frame so `required(outer)` approaches `X`. | Bounded by `max_fee`; the sponsor chose it. Padding raises the inner's own byte cost? No — the sponsor pays all bytes. **Residual**: a sponsor must set `max_fee` close to the expected size; the SDK computes it from the signed inner frame, so the user cannot change the inner after sponsorship without invalidating `txid`. |
| P-3 | **Cross-chain replay** of a testnet sponsorship on mainnet. | Chain tag in the sponsor's signed bytes (ADR-036). |
| P-4 | **Strip attack**: a relay removes the outer frame; the inner has no fee and is refused — but the sender's nonce? | Refused at admission, nothing applied, nonce intact. |
| P-5 | **Add attack**: a relay wraps a normal fee-paying frame in a sponsorship it signs, making the sponsor pay for a stranger. | Inner with a fee output is refused; the sponsor's own signature is needed anyway. |
| P-6 | **Mempool exhaustion**: 10,000 frames each sponsored by an address holding one fee's worth. | Per-sponsor pending cap 64 and `Σ max_fee <= balance` at admission; sponsored frames evicted first. |
| P-7 | **Sponsor rug**: the sponsor signs, then spends its balance to zero; every sponsored frame in the mempool is now invalid. | Frames are refused at apply (not the sender's nonce); the mempool re-checks on balance change; the user's inner frame can be re-sponsored (new outer, same inner). |
| P-8 | **Stale sponsorship**: a frame signed a year ago surfaces. | `valid_until` and `MAX_SPONSOR_WINDOW` at admission. |
| P-9 | **Depth bomb**: v13 inside v13 inside v13. | Depth 1; inner v13 refused at decode. |
| P-10 | **Vault sponsor drains itself past its window** because the fee debit is "not an output". | The debit is an output to the collector above the 4× rule (invariant 32), counted. |
| P-11 | **Fee-split bypass**: the sponsor's fee skips the base-fee burn. | Debit routes through the same `fees.rs` path; vector pins burn and tip. |
| P-12 | **Two sponsors, one inner**: both frames in the mempool, both in a block. | Second fails on the nonce; the first sponsor pays once. Outer txid differs so gossip dedups neither; mempool keeps one per inner txid (lowest outer bytes wins). |
| P-13 | **Sponsored frame carries a v9 session-key inner** so a session key spends through a sponsor beyond its daily window. | The inner's own policy rules apply unchanged; sponsorship changes who pays the fee, not what the inner may do. |

## Mitigations

As the table. The consensus rules are the six in ADR-135 §P-4; the mempool
rules (P-6, P-7, P-12) are node-local policy and must not be written as
validity checks (security-architect S-5).

## Replay tests (`crates/node/tests/sponsored_fee_tests.rs`)

1. `a_sponsorship_applied_to_a_second_inner_with_the_same_txid_fails_on_the_nonce`
2. `a_sponsorship_cannot_pay_more_than_max_fee_for_a_padded_outer_frame`
3. `a_sponsorship_signed_under_another_chain_tag_is_refused`
4. `a_stripped_outer_frame_leaves_an_unfunded_inner_that_is_refused_without_a_nonce_burn`
5. `wrapping_a_fee_paying_inner_in_a_sponsorship_is_refused`
6. `the_sixty_fifth_pending_frame_per_sponsor_is_dropped_at_admission_but_valid_in_a_block`
7. `a_sponsor_that_spends_to_zero_voids_its_frames_at_apply_without_consuming_the_senders_nonce`
8. `a_sponsorship_past_valid_until_or_beyond_the_window_is_refused`
9. `a_v13_frame_inside_a_v13_frame_is_refused_at_decode`
10. `a_sponsor_vault_counts_the_fee_toward_its_window`
11. `base_fee_burn_and_tip_split_are_unchanged_for_a_sponsored_frame`
12. `two_sponsors_of_one_inner_pay_once`
13. `a_sponsored_session_key_frame_still_obeys_its_scope_and_window`
14. `a_v13_frame_is_refused_by_verify_and_before_activation_by_verify_at`
15. Attacknet (plan V1): 1,000 sponsored frames from 20 sponsors with
    balances below their ceilings; assert the mempool never admits more
    than the balance covers and no sender nonce is lost.
