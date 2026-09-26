# Grants program: design for approval

**Status: DRAFT, waiting for `APPROVED: grants program`.** Nothing here
spends, announces or opens applications. The amounts are left for the owner
to set: a grant budget is a treasury decision, not an engineering one.

## Funded by the treasury rules (Master Prompt 18)

Every milestone payment is a `treasury` spend. That gives it three things:

- **Staged approval.** Two stages: the grants committee, then a community
  veto window (`Treasury::new(.., stages = 2)`). A spend executes only when
  its last stage approves.
- **A per-epoch limit.** The program cannot pay more than its epoch budget,
  whatever the committee approves. `reference-apps::dao` tests the same
  limit against a captured vote.
- **A public ledger.** Each proposal, approval, rejection and execution is an
  `Entry` with its height. The ledger is the public report.

Long grants to individuals vest (`treasury::vesting::Grant`), with a cliff,
linear release, termination, and clawback only where the contract says so.

## Milestones

| Stage | Paid | Evidence required |
|---|---|---|
| Kick-off | 10% | Signed scope with measurable deliverables |
| Each milestone | agreed share | A merged PR, deployed code or a published report, reviewed by a named committee member |
| Completion | the remainder | A public write-up, plus 30-day maintenance |

Rules:

- Nothing is paid for a promise.
- A missed milestone pauses the grant. It is not cancelled automatically, and
  the ledger shows why.

## Public reporting

- Every grant appears in the ledger from proposal to completion.
- A quarterly summary is generated from the ledger, not written by hand. It
  shows amount committed, amount paid, milestones hit and missed, and the
  outcome metrics below.

## Outcome metrics

These use the same methods as [METRICS.md](METRICS.md):

- Grantee repositories still active after 90 days.
- Contracts deployed by grantees and used by others.
- Developers who stay after 30 and 90 days.

## Priority areas

In the order the ecosystem is blocked:

1. The `caller` host function and its audit (ADR-026). It unblocks every
   contract template.
2. A fungible-token contract standard.
3. Wallet adoption of the design system and the MP22 clear-signing review.
4. SDKs: Go is a README only (`reports/24-devx.md`).

## Parameters for the owner to set

The following need an owner decision before the program can open:

- total budget, and budget per epoch;
- committee members;
- veto window length;
- maximum grant size.
