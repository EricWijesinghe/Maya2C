# Threat model: smart-account policy, session keys and daily limits (v4, P-5)

ADR: [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-5 (design text in ADR-107, PR #465; allow-list and large-transfer delay cut). Builds on PR #447 (wire v9, `ka:`).
Written 2026-11-12. Tests: pending (build job I5).

## Attacker

The holder of a session key (an app, a point-of-sale device, an AI agent)
that has turned hostile or been stolen; a block producer; a relay; the
account owner's rival guardians (PR #447 recovery); and a node that
verifies v9 frames with stale state.

## Assets

- The account's native balance beyond the session scope.
- The owner key set and threshold (`ka:a:`): a session key must never
  change it.
- The daily window sum (`ka:w:`): must be the same on every node.
- Bounded verification work (`MAX_VALIDATION_COST`).

## Attacks

| # | Attack | Path |
|---|---|---|
| K-1 | **Escalation**: a session key signs `AddSessionKey` for itself with unlimited scope, or `RevokeSessionKey` on a rival. | Sub-ops are owner-threshold actions only. |
| K-2 | **Kind escape**: a session key signs a `RegisterAsset`, `CreateSchedule` or guardian-recovery kind. | `kind == Transfer` only. |
| K-3 | **Recipient escape**: an output to a listed recipient plus a second output to the attacker. | Every output recipient must be listed. |
| K-4 | **Amount split**: ten transfers at `max_amount` each in one block. | Daily window caps the sum regardless of count; `max_amount` is per output. |
| K-5 | **Window straddle**: owner-signed frames and session frames each stay under the limit but together exceed it. | Window evaluated for every v9 frame of a limited account (S-9). |
| K-6 | **Window reset by reorg**: a reorg drops the block that wrote `ka:w:`; the replayed frames double-spend the window. | `ka:w:` is an overlay record under the undo journal; replay re-evaluates against the restored sum. |
| K-7 | **Mempool/apply split**: the mempool sees `spent = L - 1`, the block sees `spent = L` after an earlier frame in the same block. | Both evaluate against the parent state *plus* earlier frames in the same block (the block builder applies sequentially; the mempool admits optimistically and the block refuses the loser). Refusal is not a nonce burn. |
| K-8 | **Verification bomb**: a v9 frame with 40 signatures of the most expensive suite. | `verify_cost` charged against `MAX_VALIDATION_COST = 40` before any check; fails closed. |
| K-9 | **Stale session row**: an expired key's row is read by a node that compares `expires_at` with the wrong height. | Height is the block's; `height <= expires_at`; vector at the boundary. |
| K-10 | **Sponsor combo**: a session key with a zero-balance account spends through a sponsor to avoid the fee reducing its window. | The fee is the sponsor's output, not the account's; the window counts the account's outputs; scope unchanged. |
| K-11 | **Vault and policy disagree**: an address that is both; the policy allows, the vault refuses, or vice versa. | Stricter wins; both checked; vector. |
| K-12 | **Key-hash preimage**: `ka:s:` keyed by `key_hash`; an attacker registers a session key whose hash equals the owner key's hash. | BLAKE3; the suite byte is in the hash preimage; collision is the hash's security. |
| K-13 | **Bit-order activation**: `ACCOUNT_POLICY` (45) active while `ACCOUNT_AUTHORITY` (9) is not. | Entry admission refuses (S-10). |

## Mitigations

As the table. The design's core property: a session key is a *weaker*
owner, never a different one, and every rule is a subset check on the
frame's outputs and kind evaluated by one function with one state.

## Replay tests (`crates/node/tests/account_policy_tests.rs`)

1. `a_session_key_cannot_add_or_revoke_a_session_key_or_change_the_owner_set`
2. `a_session_key_can_sign_only_a_transfer_kind`
3. `a_frame_with_one_unlisted_recipient_is_refused_whole`
4. `ten_transfers_at_max_amount_stop_at_the_daily_window`
5. `owner_and_session_frames_share_one_daily_window`
6. `a_reorg_restores_the_window_sum_and_replayed_frames_re_evaluate_against_it`
7. `two_frames_that_each_fit_but_together_exceed_the_window_apply_one_and_refuse_one_without_a_nonce_burn`
8. `validation_cost_is_charged_before_any_signature_is_verified_and_fails_closed_at_41`
9. `a_session_key_at_expires_at_signs_and_one_block_later_cannot`
10. `a_sponsored_session_frame_counts_its_outputs_not_its_fee_toward_the_window`
11. `a_vault_that_is_also_a_policy_account_obeys_the_stricter_limit`
12. `an_account_policy_entry_without_account_authority_active_is_refused`
13. `a_v9_frame_is_refused_by_verify_and_by_verify_at`
14. Mutation check (as PR #447 did): accept an unlisted recipient; record
    which tests fail (expect 3, 10).
