# ADR-135 amendment (2026-11-23): sponsored fees as built (P-4, V4-I4)

**Status:** Proposed (amends ADR-135 P-4; claim `135-I4` in the ADR claims file, no new number). Supersedes [ADR-106](ADR-106-sponsored-fees.md) on `master`.
**Date:** 2026-11-23

ADR-135 is on `docs/adr-135-v4-umbrella` (PR #477) and not on this branch's
base, so the PR that builds P-4 (#501) carries its own design record, as #548
does for P-5. When #477 lands, this text is appended to ADR-135's amendment
section and this file is removed. Decided under Eric 2026-11-14 ("I approve
everything, proceed"); DECISIONS.md has the entries (2026-11-18, 2026-11-22,
2026-11-23). The signed-bytes change (M1) is flagged there for Eric's explicit
review before the activation height is written.

## ADR-106 is superseded

ADR-106 (2026-11-12) proposed the same design at wire **v11**. ADR-135
released v11 and reserved **v13** (`0x0D`) for sponsorship; nothing ever
shipped at v11. What ADR-135 P-4 keeps from ADR-106: an outer frame around an
unchanged inner, the sponsor signs the inner's txid, `max_fee` and
`valid_until` under the chain tag, the debit happens at apply, the inner
carries no fee output, a 86,400-block admission window. What changed is below.
When this branch meets `master`, ADR-106's status line becomes "Superseded by
ADR-135 P-4 (this amendment)".

## Decisions as built

1. **Wire v13**, layout SP-1 (`spec/02-transactions.md`). A sponsored frame is
   never an inner at any depth; a later wrapping version must refuse a `0x0D`
   inner the same way.
2. **SP-2 signs `inner_len`** (M1, 2026-11-18). A multisig inner's approvals
   are outside its txid, so without the length a relay could swap a
   mixed-suite quorum for a longer one and grow the fee the sponsor pays. The
   domain stays `maya2c/sponsor/v1`: the form was never active anywhere.
3. **The fee is `required(outer_len)`, no surcharge** (Eric O-4, ADR-106
   open question 1), debited by one checked subtraction, burned like an
   unsponsored fee; a vault sponsor's debit counts against its window
   (invariant 32).
4. **Inner versions v5-v8** on this branch (ADR-135's table says v6-v9; v9 is
   not on this branch).
5. **Mempool policy, not validity:** 64 pending frames per sponsor, their
   `max_fee`s plus the sponsor's own pooled spends within its balance; one
   frame per inner, the first seen kept, a later one refused as "already
   sponsored"; the vault window dry-run on admission and revalidation.
6. **Bit 44 is held in code** (2026-11-23). `EffectiveSchedule::admit`
   refuses a governance or council entry naming bit 44 while X1 and M3 are
   open. Genesis is not covered: its schedule is a reviewed file.

## Open, each blocking the activation height

- **X1**: a sponsor with a `ka:a:` record or a `ka:w:` row (ADR-086 Rule A,
  AA-9) must be refused; the records live on #548, so the test is written on
  the postgen/v4 integration. No test exists yet.
- **M3** for the other v4 forms (#487, #493, #502, #548): measured and, if
  quadratic, staged in place as sponsored transfers now are.

## Evidence

`crates/node/tests/v4_redteam_sponsored.rs`, `sponsored_fee_tests.rs`,
`sponsored_conformance.rs`, `spec/tests/sponsored.json`; run output on PR #501.
