# ADR-135 amendment (2026-11-14): smart-account policy as built (P-5, V4-I5)

**Status:** Proposed (amends ADR-135 P-5; claim `135-I5` in the ADR claims file, no new number)
**Date:** 2026-11-14

ADR-135 itself is on `docs/adr-135-v4-umbrella` (PR #477) and not on this
branch's base (`postgen/account-authority`, PR #447). This file holds the
amendment so the PR that builds P-5 carries its own design record. When #477
lands, this text is appended to ADR-135's "Amendment" section and this file is
removed. Decided under Eric 2026-11-14 ("I approve everything, proceed");
DECISIONS.md has the entry.

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
   sweep, and a key stays void if the same digest later returns (A→B→A).
   Without this a thief holding the owner key could add a session key for
   himself and keep spending after the guardians recovered the account.
   Re-adopting the same policy is therefore also a way to void every session
   key at once. Tests: `an_authority_change_voids_every_session_key`,
   `a_voided_session_key_stays_void_when_the_old_authority_returns`.
2. **`ka:w:` is one row per account**, `limit ‖ window_start ‖ spent`, not
   `ka:w:‖account‖window_start` per window. The per-window key left one stale
   row per account per window, forever. This also keeps the limit out of
   #447's `ka:a:` record encoding.
3. **The cap is on the frame, not the output, and counts what the vault
   counts** (`vault_outflow`, invariant 32): every output, collector outputs
   up to the required fee exempt. A per-output cap could be split across two
   outputs. The fee collector is always an allowed recipient, so a session
   frame can pay its fee. Tests:
   `with_fees_on_a_session_frame_pays_its_fee_outside_its_cap_and_recipients`,
   `with_fees_on_the_required_fee_is_outside_the_window`.
4. **Window = 86,400 blocks**, not 43,200: one day at ADR-135 O-1's answer,
   "1 block/s", recorded in ADR-135's "Eric's answers (2026-11-12)" table
   (branch `docs/adr-135-v4-umbrella`, PR #477). 43,200 was the
   smart-account crate's 2 s figure.
5. **Methods are plain transfers only.** The V4-I5 brief asked for "allowed
   recipients/methods". P-5 binds the method to `Transfer` (S-1), and that is
   what was built: a method allow-list for contract calls would let value
   move where the cap does not see it. A per-kind allow-list is a later ADR.
   **This is a scope reduction from the brief and needs Eric's explicit
   confirmation.**
6. **Expired rows are kept until revoked or replaced.** P-5 says an expired
   `ka:s:` row is deleted "by the next `RevokeSessionKey` or by any
   owner-signed frame from that account touching it". Owner frames do not
   read `ka:s:` rows, so "touching it" names no code path; cleaning up on an
   owner frame would add an unbounded scan or a second key index. As built:
   `RevokeSessionKey` deletes a row whether live, expired or voided, and an
   `AddSessionKey` with the same `key_hash` overwrites it; nothing else
   deletes one and no sweep runs. The storage is paid by the byte fee of the
   `AddSessionKey` that wrote it.
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
   immediate), so against a compromised owner key the window is still
   advisory; it is a bound on what a leaked session key and the owner's
   ordinary frames can move. Tests:
   `a_limited_account_cannot_move_value_without_outputs`
   (`v4_redteam_accounts.rs`), vector
   `AA-window-refuses-a-kind-without-outputs`. Decided under Eric
   2026-11-14.
8. **`SetDailyLimit` stays halted by the circuit breaker**, while
   `RevokeSessionKey` is not. Considered: exempting a `SetDailyLimit` that
   lowers a non-zero limit, as a defensive move. Not done: the breaker's
   module map is a function of the transaction alone, and whether a new
   limit is lower depends on state; exempting every non-zero `SetDailyLimit`
   would let an owner-key thief raise the limit during a halt. Revoking the
   leaked session key is the defence that is never halted.
9. **A per-frame cap with no window has no total bound.** `max_amount` caps
   each frame; without a daily limit a leaked key can sign one frame per
   nonce until `expires_at`. The clear-signing text of `AddSessionKey` says
   so. Whether `AddSessionKey` should require a window is left open for the
   ADR-135 review.

Also as built: the verification budget (40 units) applies to session-signed
frames only, so #447's 3-of-5 hybrid owner frames (55 units) are unchanged;
add refuses `expires_at` beyond one year (`MAX_SCHEDULE_HORIZON_BLOCKS`) and a
session key equal to the authority digest; a schedule naming bit 45 before
(or without) bit 9 is refused (S-10).

## Alternatives considered

- For 1, a per-account counter bumped on every authority change: one more
  field in #447's `ka:a:` record encoding for the same effect. The record's
  existing `since` height already identifies the generation.
- For 6, implementing owner-frame cleanup: see 6.

## Consequences

- `ka:s:` rows are 8 bytes longer than P-5's layout.
- Bounded work per frame, by inspection (no test counts storage accesses):
  at most three policy record reads (`ka:a:`, `ka:s:`, `ka:w:`) and one
  `ka:w:` write, a membership test over at most 16 recipients per output, and
  the 40-unit budget checked before any signature. No end-of-block step.
