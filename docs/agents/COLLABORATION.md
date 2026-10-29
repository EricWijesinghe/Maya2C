# Agent collaboration: Claude (lead) and Gemini (worker)

Set 2026-10-24 at Eric's direction. Eric stays the owner; Claude directs
Gemini's work inside the limits below. This file is the contract both read at
the start of a session.

## Roles

- **Claude (lead):** writes the task queue, reviews Gemini's PRs against it,
  owns consensus, cryptography, staking and the gate-8/gate-10 work.
- **Gemini (worker; Jules, Gemini CLI or Antigravity):** takes bounded tasks
  from the queue below. It may not pick its own work or widen a task.
- **Eric:** decides money, economics, mainnet, block-format changes, keys,
  publishing, and anything marked "Eric" in CLAUDE.md ("When to stop and ask").

## Rules

1. One git worktree and one branch per agent (`gemini/<task>`); never two
   agents in one checkout. All changes land as PRs; CI is the gate.
2. Gemini does not touch: `crates/node/src/consensus`, `crates/staking`,
   `crates/crypto-pq`, `ledger-math`, `dex`, `governance`, `fee-market`,
   `vrf`, genesis files, or anything under `invariants.md`. A PR that does is
   closed unread.
3. No new dependency, no `unsafe`, no float/HashMap-order/wall-clock in
   anything consensus-adjacent (CLAUDE.md Standing Order 4).
4. Secrets: neither agent reads, prints or commits keys, passphrases, tunnel
   credentials or SSH keys. Gemini gets none of them.
5. Nothing that costs money, touches the live servers, or publishes runs
   without Eric's `APPROVED: <step>`.
6. Numbers in a PR come from real command output pasted in the description.
   No "zero warnings", benchmark or test-pass claim without it (Standing
   Order 1).
7. Claude never signs off its own consensus/crypto change; it gets the
   rust-reviewer + security-reviewer pass and Eric's review.
8. The word "audited" or "independent" is not used until an external audit
   exists (Standing Order 10).

## Task queue (Gemini)

Each task: small, testable, no overlap with the forbidden paths.

1. Explorer: validators page (committee, jailed peers, bonds) in
   `apps/explorer`; tests for the new route.
2. Docs: runbook for "PC peers fell behind: snapshot restore", from
   `D:\Maya2C-validators\pc\start-pc-validators.ps1` behaviour.
3. Tests: add missing unit tests in `apps/faucet`, `apps/status`
   (coverage floor 80%).
4. Site: Cloudflare/DNS hardening checklist as docs (no live changes).
5. Dependency hygiene: non-major bumps only, one PR each, `cargo deny` clean.

## Claude's queue (not for Gemini)

1. Gate 8: catch-up for a validator beyond the engine window (ADR-038).
2. Gate 10: followers that all need to catch up (ADR-041; needs Eric).
3. Self-run security review and red-team pass on the 5-validator committee.

## Review loop

Gemini opens a PR → Claude checks it against its task and the rules → CI
green → Eric merges. A PR that fails a rule is closed with the rule number.
