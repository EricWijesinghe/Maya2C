# Beat-bar status: draft for publication

**Status: DRAFT, waiting for `APPROVED: publish beat-bar table`.**

The bars are the ones in [docs/strategy/BEAT_BARS.md](../strategy/BEAT_BARS.md)
(Master Prompt 21). This table only adds a status, updated each release:

- **met**: Maya2C's figure beats the bar and has been reproduced outside the
  project;
- **not yet met**: covers ties, worse figures and anything unmeasured;
- **regressed**: worse than the previous release's measured figure.

**Release: none yet** (the `claude/task-0g86kl` branch, 2026-09-26). No bar is
met. Nothing can have regressed, because there is no earlier measured release
to compare against.

| Weakness | Bar | Maya2C now | Status | Changed since MP21 |
|---|---|---|---|---|
| Finality | p99 time to irreversibility | probabilistic PoW, 24 confirmations = 6 min at q = 0.3 | not yet met | — |
| Quantum readiness | share of value behind PQ signatures from genesis | 100% hybrid | not yet met (**ties** QRL) | — |
| Node cost | full-node disk | 3.4 TB/month at 100 TPS, unpruned | not yet met (**worse**) | — |
| Liveness | longest halt in 24 months | no public network | not yet met (unmeasurable) | — |
| Blind signing | share of transactions rendered in plain language | every effect kind renders (`clear-sign`), and 220 of 220 synthesized drainer cases are flagged. No wallet ships it | not yet met | renderer covers all effect kinds; native coin now named, not shown as "token 0000…" |
| New-user time to first transfer | minutes | no study | not yet met | — |
| Bridge trust | parties that must be honest | no bridge | not yet met | — |
| Exploit losses | value lost per value secured | no value secured | not yet met (unmeasurable) | — |
| Developer time to a tested contract | minutes | one agent port, 81 s, of an ownership contract that cannot be secured (ADR-026). No human measured | not yet met | first measured port |

## Publishing rule

This table goes out as it stands, rows marked "not yet met" included. A
version that drops the rows Maya2C loses is not this table.
