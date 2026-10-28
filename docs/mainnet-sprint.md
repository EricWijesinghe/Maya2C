# Mainnet sprint: 2026-10-28 to 2026-11-28

Eric, 2026-10-28: mainnet within one month, everything authorised. The
re-genesis, signing the genesis and the launch itself are still asked for by
name (`APPROVED: <step>`), as is anything that costs money, touches keys or a
live server. "Everything authorised" does not stand in for those.

Gates: `docs/site/src/data/mainnet-gates.ts` (6 of 10 met on 2026-10-28:
1 replay, 2 shielded off, 3 seven days without a halt, 8 rejoin, 9 bounded
logs, 10 halt recovery). Open: 4 separate validators, 5 ceremony,
6 economics, 7 CI and release.

## Calendar

| Week | Dates | Done when |
|---|---|---|
| 1 | 10-28 to 11-04 | Every consensus-format change merged, then **code freeze**: #223, the certs.log fix, #218, #221, #222, #205, #206, 24 decimals, the 512-bit product audit, the MINE-* spec rules |
| 2 | 11-05 to 11-11 | Re-genesis on the mining build (`APPROVED: re-genesis`); Hub mines end to end against it; attacknet running |
| 3 | 11-12 to 11-18 | Second 7-day soak on the mining build, alongside the attacknet; self pre-audit and its fixes |
| 4 | 11-19 to 11-28 | Genesis ceremony (`APPROVED: sign genesis`), tagged release built by CI, launch (`APPROVED: mainnet`) |

ADR-062a/b (incremental root, per-validator staking records) are deferred
past launch: the measured root cost is under 7 ms at 10k accounts, and the
sparse tree is behind an activation height, so it needs no re-genesis.

## Lanes

Two sessions, one worktree each; neither touches the other's lane without
asking.

| Lane | Owns |
|---|---|
| Economics and genesis | #218 u256, #221 forever tail, #222 ML-DSA-87 keys, 24 decimals, re-genesis, status, site, testnet operations; gates 5 and 6 |
| Consensus hardening and release | #223 lost-fetch fix, #228 certs.log fix, `envelope.from` binding, #205, #206; CI throughput (#227), release pipeline and Hub installers (#224); Hub end to end; attacknet and self pre-audit; gate 7 |

## Rules for the month

- One heavy cargo job on this machine at a time; say so before starting one.
- One target directory per worktree: a shared one links another branch's
  crates and passes tests against source nobody is looking at.
- Freeze means no consensus-format change after 11-04 without a written
  reason in DECISIONS.md.
- A number in any public copy is measured, and a "met" gate names its
  evidence.
