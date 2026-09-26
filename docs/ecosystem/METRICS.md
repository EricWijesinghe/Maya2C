# Ecosystem metrics: methods and current values

Master Prompt 30 §4 asks for a public dashboard. `cargo xtask eco-metrics`
computes every metric this repository can support and names a gap for each
one it cannot. **It is not live on testnet data, because there is no testnet.**
The output is counts only, with no email addresses, so it can be published
as it stands.

## Methods

| Metric | Method | Why this method |
|---|---|---|
| Monthly active developers | Distinct commit author emails per UTC calendar month. Agent and bot identities (`noreply@anthropic.com`, `[bot]`, `noreply@github.com`) are counted separately and never as developers | Commits are the one activity signal that cannot be bought with a click. Counting AI agents as developers would inflate the figure with no human behind it |
| Retention at 30 / 90 days | Of human authors whose first commit is at least N days before the newest commit, the share who committed again at least N days after their first | This excludes authors too new to judge, so a burst of newcomers does not dilute the rate |
| Contracts deployed and used | Contract crates in `contracts/` today. On a network: deployments, and contracts called by more than one account in 30 days | "Deployed" alone rewards spam deployments; "used by others" does not |
| Time to first deploy | Measured per participant by the course's beginner protocol ([COURSE.md](COURSE.md)) | It needs people, not logs |
| SDK downloads | Registry download counts once published | Nothing is published: publishing needs approval |
| Grant outcomes | From the treasury ledger ([GRANTS.md](GRANTS.md)) | The program is not approved |

Scope: "Maya2C-related repositories" is this repository for now. Adding other
repositories means passing their history to the same method, and none exists
yet.

## Current values

From `cargo xtask eco-metrics`, 2026-09-26, in this session's clone:

| Metric | Value | Source |
|---|---|---|
| Active developers, 2026-09 | 1 human, 1 agent identities | git history |
| History covered | 77 commits over 13 days (**shallow clone: older history missing**; run in a full clone) | git history |
| Retention at 30 days | not yet measurable: no author's first commit is 30 days old | git history |
| Retention at 90 days | not yet measurable: no author's first commit is 90 days old | git history |
| Contract crates in tree | 2 | contracts/ |
| Contracts deployed and actively used | **gap**: no public testnet to read | — |
| Time to first deploy | **gap**: no deploy flow outside tests; the course's first cohort measures it | — |
| SDK downloads | **gap**: no SDK is published (publishing needs approval) | — |
| Grant outcomes | **gap**: the grants program is not approved | — |

The clone this ran in is shallow, so the history figures are a lower bound. A
full clone gives the real ones.
