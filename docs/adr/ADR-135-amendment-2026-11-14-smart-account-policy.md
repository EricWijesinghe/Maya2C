# ADR-135 amendment (2026-11-14): smart-account policy as built (P-5, V4-I5)

**Status:** Proposed (amends ADR-135 P-5; claim `135-I5` in the ADR claims file, no new number)
**Date:** 2026-11-14

ADR-135 itself is on `docs/adr-135-v4-umbrella` (PR #477) and not on this
branch's base (`postgen/account-authority`, PR #447). This file holds the
amendment so the PR that builds P-5 carries its own design record. When #477
lands, this text is appended to ADR-135's "Amendment" section and this file is
removed. Decided under Eric 2026-11-14 ("I approve everything, proceed"),
except the items marked **PENDING Eric**, which that approval does not cover;
DECISIONS.md has the entry. Ten decisions below: 1-4, 6-8 and 10 decided
under that approval; 5 and 9 **PENDING Eric**.

## Context

The build of P-5 (bit 45, `state::account_policy`) found places where P-5
could not be built as written, or where a stricter rule was safer. Standing
Order 9: each is recorded, not worked around. Spec rules AA-7..AA-10
(`spec/02-transactions.md`), vectors `spec/tests/account-policy.json`, tests
`crates/node/tests/account_policy_tests.rs`.

## Decision

1. **A session key is bound to the authority generation that granted it.**
   Not in P-5. Each `ka:s:` row stores `granted_by` (the authority digest in
   force) and `granted_since` (the height that authority took effect). The
   key counts only while both match the authority in force. Recovery or a
   matured `AdoptPolicy` voids every older session key in O(1), with no
   sweep, and a key stays void if the same digest later returns (A→B→A) at
   a later height. The generation is per height: two immediate authority
   changes inside one block (A→B→A with no guardians, both taking effect at
   the same height `h`) give back `(A, h)`, and a key granted at `(A, h)`
   earlier in that block counts again. Only the owner key can do that, so it
   is not a theft path; a per-account counter bumped on every authority
   write would close it (see Alternatives) and is the upgrade if the case
   ever matters.
   Without this a thief holding the owner key could add a session key for
   himself and keep spending after the guardians recovered the account.
   Re-adopting the same policy is therefore also a way to void every session
   key at once. Tests: `an_authority_change_voids_every_session_key`,
   `a_voided_session_key_stays_void_when_the_old_authority_returns`.
2. **`ka:w:` is one row per account**, `limit ‖ window_start ‖ spent`, not
   `ka:w:‖account‖window_start` per window. The per-window key left one stale
   row per account per window, forever. This also keeps the limit out of
   #447's `ka:a:` record encoding.
3. **The cap is the key's total, not a frame's or an output's, and counts
   every output, the fee included.** A per-output cap could be split across
   two outputs, and a per-frame cap across two frames (see 9). The fee
   collector is always an allowed recipient, so a session frame can pay its
   fee, but the fee is charged to `max_amount`: `max_amount` bounds what the
   key can cost the account, fees included. A session frame must also pay
   the listed recipients a non-zero total. Changed in the PR #548 review: as
   first built the cap counted what the vault counts (`vault_outflow`,
   collector outputs up to the required fee exempt), so a leaked key at its
   cap could still sign frames that moved only the required fee (or 0-value
   outputs), burning the balance in base fees at every nonce until
   `expires_at`, and taking every nonce so the owner's `RevokeSessionKey`
   kept going stale. With the fee charged and a non-zero payment required, a
   used-up key signs nothing, and with fees off it cannot take nonces for
   free. Considered instead: a per-key fee budget in `SessionScope` (changes
   the action encoding for the same effect). The window (AA-9) and the vault
   keep the required-fee exemption: they bound the owner's own frames, which
   must always be able to pay a fee. Tests:
   `with_fees_on_a_session_frames_fee_counts_against_its_cap`,
   `with_fees_on_the_required_fee_is_outside_the_window`,
   `a_session_key_at_its_cap_cannot_burn_the_account_in_fees`,
   `a_session_frame_paying_nobody_cannot_take_nonces_with_fees_off`; vectors
   `AA-session-fee-counts-against-cap`, `AA-session-pays-no-recipient`.
   Decided under Eric 2026-11-14.
4. **Window = 86,400 blocks**, not 43,200: one day at ADR-135 O-1's answer,
   "1 block/s", recorded in ADR-135's "Eric's answers (2026-11-12)" table
   (branch `docs/adr-135-v4-umbrella`, PR #477). 43,200 was the
   smart-account crate's 2 s figure.
5. **Methods are plain transfers only.** The V4-I5 brief asked for "allowed
   recipients/methods". P-5 binds the method to `Transfer` (S-1), and that is
   what was built: a method allow-list for contract calls would let value
   move where the cap does not see it. A per-kind allow-list is a later ADR.
   **PENDING Eric.** This is a scope reduction from the brief ("allowed
   recipients/methods"), so the 2026-11-14 approval does not cover it; it
   needs Eric's explicit confirmation before bit 45 is scheduled. If he
   wants methods, the follow-up is a per-kind allow-list in the scope, each
   kind admitted only with a rule for how its debit is charged to the cap.
6. **Expired rows are kept until revoked or replaced.** P-5 says an expired
   `ka:s:` row is deleted "by the next `RevokeSessionKey` or by any
   owner-signed frame from that account touching it". Owner frames do not
   read `ka:s:` rows, so "touching it" names no code path; cleaning up on an
   owner frame would add an unbounded scan or a second key index. As built:
   `RevokeSessionKey` deletes a row whether live, expired or voided, and an
   `AddSessionKey` with the same `key_hash` overwrites it; nothing else
   deletes one and no sweep runs. The storage is paid by the byte fee of the
   `AddSessionKey` that wrote it. No cap on live `ka:s:` rows per account
   exists, so the rows an account holds grow with every distinct key it adds
   and does not revoke; each one costs only that account its byte fee, and
   per-frame work does not depend on the count (one keyed read). A cap needs
   a per-account count kept in another record, which changes an encoding.
   Upgrade trigger: decided with decisions 5 and 9, before bit 45 is
   scheduled, or sooner if a state-size sweep shows `ka:s:` among the
   growing prefixes. The shortcut is marked in `add_session_key`.
7. **A limited account sends only plain transfers and account actions.**
   The window counts output-carried value (decision 3). As first built,
   kinds that move value without outputs (channel funding, stake locks, DEX
   orders, HTLC locks, contract calls with value) were allowed and not
   charged. The V4-V1 red team showed that on the full v4 fork this lets an
   owner frame move a limited account's whole balance in one block:
   `CreateSchedule` with `first_due = height + 1` and `amount = balance`
   (#502), or a channel to a colluding counterparty today. Two fixes were
   weighed: charge each such kind's debit to the window, or refuse those
   kinds while a limit is set. Chosen: refuse (AA-9), the rule a vault
   account already follows (ADR-030), because it is fail-closed. A kind
   added later, such as the v4 names, NFT, asset-registration and schedule
   forms, is refused for a limited account until someone writes how the
   window charges it, rather than leaking until someone notices. The owner
   who needs one of those kinds lifts the limit first (`SetDailyLimit(0)`,
   which has no delay but, like every `SetDailyLimit`, is refused while the
   circuit breaker halts the account, decision 8), so against a compromised
   owner key the window is still advisory; it is a bound on what a leaked session key and the owner's
   ordinary frames can move. Tests:
   `a_limited_account_cannot_move_value_without_outputs`
   (`v4_redteam_accounts.rs`), vector
   `AA-window-refuses-a-kind-without-outputs`. Decided under Eric
   2026-11-14.
   Two vault actions stay open (PR #548 review): `Cancel` and `Execute`.
   Neither moves the sender's balance (`Cancel` returns escrow to the
   vault's owner, `Execute` pays escrow already debited from it), and
   `Cancel` is a guardian's one defence during a vault delay, never halted
   by the breaker. Refusing it would let a thief's request at a vault whose
   guardian has a daily limit run out its delay during a halt, since the
   halt also blocks the `SetDailyLimit(0)` that would lift the limit.
   `Request` and `Configure` stay refused. Test:
   `a_guardian_with_a_daily_limit_cancels_a_vault_request_during_a_halt`;
   vector `AA-window-admits-a-guardian-vault-cancel`.
8. **`SetDailyLimit` stays halted by the circuit breaker**, while
   `RevokeSessionKey` is not. Considered: exempting a `SetDailyLimit` that
   lowers a non-zero limit, as a defensive move. Not done: the breaker's
   module map is a function of the transaction alone, and whether a new
   limit is lower depends on state; exempting every non-zero `SetDailyLimit`
   would let an owner-key thief raise the limit during a halt. Revoking the
   leaked session key is the defence that is never halted.
9. **`max_amount` is cumulative** (changed in the PR #548 review). As first
   built it capped each frame, so without a daily limit a leaked key could
   sign one capped frame per nonce until `expires_at`, up to a year. Now the
   `ka:s:` row carries `spent`, charged in staging next to the window (one
   more write per session frame) and restored when the block builder skips
   the frame, and a frame is refused once `spent + outflow > max_amount`.
   `RECORD_VERSION` 1 is unreleased and dark, so the row changed in place.
   Re-adding the same `key_hash` is the owner's way to start a new total.
   Considered instead: refuse `AddSessionKey` without a `ka:w:` row, and
   refuse `SetDailyLimit(0)` while a live session key exists; the second
   needs a per-account key index, and the bound would still be advisory.
   Tests: `a_session_key_cannot_pass_its_cap_by_repeating_frames`,
   `a_skipped_session_frame_leaves_no_cap_charge_in_the_builder`; vector
   `AA-session-total-over-cap`. **PENDING Eric** before bit 45 is
   scheduled.
10. **Class: RESEARCH, by the activation-height reading** (Standing Order
    3). The code is reachable from consensus paths (staging, the mempool,
    the state root, the invariant guard), so it does not fit RESEARCH read
    as "nothing in consensus calls it". It fits the repo's other reading,
    "activation height `u64::MAX`": every path is gated by
    `ACCOUNT_POLICY_ACTIVATION_HEIGHT = u64::MAX` and by bit 45 in the
    upgrade schedule, so no consensus outcome on any network depends on it
    (vector `AA-root-without-policy-rows`: a chain with no policy rows keeps
    its root byte for byte). Promotion to REAL is the written decision that
    schedules bit 45 at a height, after decisions 5 and 9 are closed.
    Decided under Eric 2026-11-14.

Also as built: the verification budget (40 units) applies to session-signed
frames only, so #447's 3-of-5 hybrid owner frames (55 units) are unchanged;
add refuses `expires_at` beyond one year (`MAX_SCHEDULE_HORIZON_BLOCKS`) and a
session key equal to the authority digest; a schedule naming bit 45 before
(or without) bit 9 is refused (S-10).

## Alternatives considered

- For 1, a per-account counter bumped on every authority change: one more
  field in #447's `ka:a:` record encoding. The record's existing `since`
  height identifies the generation across heights, but not between two
  immediate changes in one block (decision 1).
- For 6, implementing owner-frame cleanup: see 6.

## Consequences

- `ka:s:` rows are 40 bytes longer than P-5's layout (`granted_since`, and
  the 32-byte `spent`).
- Bounded work per frame: at most three policy record reads (`ka:a:`,
  `ka:s:`, `ka:w:`) and two writes (`ka:s:`, `ka:w:`), a membership test
  over at most 16 recipients per output, and the 40-unit budget checked
  before any signature. No end-of-block step. The input bounds are pinned
  by tests (16 recipients admitted, 17 refused at decode; 40 units passes
  the budget, 41 refused), the read and write counts by inspection only: no
  test counts storage accesses.
- Expired and voided `ka:s:` rows are never swept and live rows are not
  capped per account (decision 6), so an account's rows grow with every
  distinct key it adds until it revokes them; the account pays for each.

## Open before bit 45 is scheduled

- Decision 5 (methods are plain transfers only): Eric's confirmation.
- Decision 9 (cumulative cap): Eric's confirmation.
- Decision 6 (no per-account row cap): decided with 5 and 9.
- Base: this PR is stacked on `postgen/account-authority` (#447), not on
  `postgen/v4` as briefed, because `postgen/v4` does not contain #447 yet.
  It is retargeted or rebased onto `postgen/v4` once #447 flows into it.
