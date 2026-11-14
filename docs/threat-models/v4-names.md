# Threat model: names (v4, P-3)

ADR: [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-3 (design text in ADR-105, PR #465; decaying premium cut).
Written 2026-11-12. Tests: pending (build job I3).

## Attacker

A squatter with many keys and native coin, a block producer watching the
mempool, a name owner, the holder of an address a name points at, and a
wallet user reading a name.

## Assets

- The mapping `label → target` that a payer trusts when typing `alice.maya`.
- The reverse mapping `address → label` that an explorer displays as
  identity.
- The commitment's secrecy until reveal.
- Scarce short labels.

## Attacks

| # | Attack | Path |
|---|---|---|
| M-1 | **Front-running the reveal**: a producer sees `RegisterName { label, salt }` in the mempool and registers first. | Producer has no aged commitment for `(label, producer, salt')`. |
| M-2 | **Reveal suppression**: a producer drops the reveal and commits itself; after `MIN_COMMIT_AGE` it registers. | The victim's commitment stays valid to `MAX_COMMIT_AGE` (86,400 blocks); the victim re-submits; the producer must win every block for 60 blocks to age its own commit first — and the victim's commit is older. Tie-break is by commitment age at registration? **No**: first valid `RegisterName` applied wins. See mitigation. |
| M-3 | **Commitment grinding**: re-submit the same commitment to keep it fresh forever, or submit thousands to pre-stake every short label. | Duplicate refused; each commit pays bytes; a commit does not reserve a label (anyone can reveal first). |
| M-4 | **Mass squatting** of 3-4 character labels at activation. | `NameBaseFee{3,4}` (O-3) and reserved labels (O-3); the chain cannot stop it by rule. |
| M-5 | **Homoglyph**: `a1ice.maya` vs `alice.maya`, `rn` vs `m`. | ASCII-only labels remove cross-script attacks; in-script confusables are the wallet's look-alike guard (ADR-105). |
| M-6 | **Reverse-record hijack**: the owner of `scam.maya` sets its reverse on a victim's address. | `SetReverse` is signed by the address; requires `target == sender`. |
| M-7 | **Target redirect after a payer resolved**: the owner changes `target` between resolution and inclusion. | Resolution is never in consensus; the transaction names the address; the wallet shows both. |
| M-8 | **Expiry sniping**: register a lapsed name the block after grace ends, before the owner notices. | Grace of 90 days, owner-only renewal in grace; after that it is free by design. |
| M-9 | **Century lock**: renew for 100 periods. | `periods <= 10`, cap at `height + 10 years`. |
| M-10 | **Stealth meta-address poisoning**: `SetStealth` with an ML-KEM key the owner does not control, so a payer sends to a stealth address the attacker can scan. | Owner-only; the owner can only hurt itself. |
| M-11 | **Stale `nm:c:` growth** as a free permanent write. | *Amended 2026-11-12 (ADR-135 amendment; built in #493).* Commitments are keyed by sender (`nm:c: ‖ committer ‖ commitment`) with an expiry index `nm:x:`; an end-of-block step prunes at most 1,024 expired commitments per block and carries the rest over, so the growth is bounded by the byte fee and the prune rate. No `CommitName` fee (Eric, O-3). The earlier "pruned by the registering transaction of the same label" could not be built: a registration cannot find another sender's commitment to the same label. |
| M-12 | **Label decode**: a label with `\0` padding, uppercase, or 64 bytes slips through a lenient decoder. | Refused at decode; vector set of refused labels. |

**M-2 mitigation, stated precisely.** The window between a commitment
ageing to `MIN_COMMIT_AGE` and its reveal is the only race. A producer that
censors a reveal and commits its own must then also censor for 60 more
blocks while the victim re-broadcasts; under ADR-040's rotating committee
that needs the attacker to hold every producer slot for 60 blocks. The
design does not tie-break by commitment age (that would let an old
commitment front-run a newer honest reveal). The residual risk is a
colluding majority of producers, which is the chain's base assumption.

## Replay tests (`crates/node/tests/names_tests.rs`)

1. `a_reveal_without_an_aged_commitment_by_the_same_sender_is_refused`
2. `a_producer_that_sees_a_reveal_cannot_register_the_label_in_the_same_block`
3. `a_censored_reveal_still_registers_once_included_within_max_commit_age`
4. `a_duplicate_commitment_is_refused_and_the_age_does_not_reset`
5. `a_commitment_older_than_max_commit_age_is_refused_and_pruned`
6. `set_reverse_requires_the_name_to_target_the_sender`
7. `transfer_clears_the_old_owners_reverse_record`
8. `only_the_owner_renews_during_grace_and_anyone_registers_after`
9. `renewal_cannot_push_expiry_past_ten_years`
10. `refused_labels_dash_a_double_dash_two_chars_64_bytes_and_non_ascii`
11. `a_reorg_restores_owner_expiry_and_the_consumed_commitment`
12. `the_root_is_unchanged_when_no_name_exists`
13. Attacknet scenario (plan V1): one producer censors reveals for 100
    blocks; assert the honest registrant wins.
