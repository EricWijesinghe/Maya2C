# 6. Staking and slashing — v0.0.1

Staking is built (`crates/staking`, ADR-028), but most of its rules have no ID
here yet: registration, delegation, rewards and the record layouts get theirs
with the per-delegation records (ADR-062 amendment, PR #555, STK-1 to STK-10).
The econ simulator (`econ/`) models validator economics for parameter choice;
it is not a consensus rule.

## Downtime and the committee floor

- **STK-11** At an epoch boundary a seated validator is down when it authored fewer than `downtime_threshold_bps` of the anchor slots it led (authored × 10,000 < threshold × expected, in integers); a validator with no slots is never down, and one still serving a jail term is unpaid but not punished again. Down validators that are eligible now are ordered worst first (authored ÷ expected, compared as authored_a × expected_b against authored_b × expected_a), ties by id. The floor keeps the last `min(down eligible, max(0, MIN_COMMITTEE − (eligible validators not down)))` of them, `MIN_COMMITTEE` = 4, counted **before** any slash. A kept validator is unpaid and not jailed, and is slashed `downtime_slash_bps` of its self bond only when what is left is at or above `min_self_bond`. Every other down validator is slashed and jailed; one already ineligible is always jailed. (ADR-070 and its amendments 2026-11-21 and 2026-11-22.) *(external vectors: crates/staking/tests/scenario.rs)*

The genesis bond margin (every mainnet seat bonds `min_self_bond` plus 10.6%,
ADR-070 amendment 2026-11-22) is a launch policy checked by
`cargo xtask genesis-check`, not a consensus rule: the node does not check
the margin.
