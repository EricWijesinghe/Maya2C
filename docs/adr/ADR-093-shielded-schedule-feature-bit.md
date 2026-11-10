# ADR-093: The shielded pool as a schedule-activated feature bit

**Status:** Proposed, 2026-11-10. **Post-genesis** consensus change: drafted
for review, not merged before mainnet.
**Date:** 2026-11-10
**Related:** [ADR-037](ADR-037-mainnet-shielded-off.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), invariants 24, 25, 34; `crates/node/src/upgrade.rs`, `crates/node/src/state/upgrades.rs`, `crates/node/src/consensus/chain.rs`, `bins/maya2c-node/src/main.rs`; spec CON-14, CON-15

## Context

ADR-037 made the pool's activation a genesis parameter and launched mainnet
with it at `SHIELDED_NEVER`, saying the pool would come on later "at a
scheduled height, through a protocol upgrade". ADR-082 then built that
schedule, but gave it no shielded feature: `FeatureSet` had bits 0–7, and
`Chain::context_at` took the shielded height from the genesis alone. So a
chain launched with the pool off had no way to turn it on short of a new
genesis.

## Decision

- **`FeatureSet::SHIELDED = 1 << 8`**, appended (bit positions are consensus).
  `KNOWN = (1 << 9) - 1`. Bit 9 is reserved for another branch and is not
  defined here.
- **`FeatureActivations::shielded`**, filled by `UpgradeSchedule::features`
  like every other bit; `SHIELDED_NEVER` when no entry carries it.
- **One source wins, two are refused** (`resolve_shielded_activation`, CON-14).
  The genesis height and the schedule's height are read together:
  genesis not `SHIELDED_NEVER` and no entry → the genesis height; genesis
  `SHIELDED_NEVER` and an entry → the entry's height; both → refused with the
  typed `NodeError::ShieldedActivationConflict`. An absent genesis field means
  height 0, so it counts as set. The refusal happens at genesis validation
  (`GenesisConfig::validate`, i.e. at node startup) for genesis entries, and
  in `Chain::context_at` for any other path.
- **The writer refuses the entry** (`check_shielded_entry`, CON-15), inside
  `EffectiveSchedule::admit`, which both governance and the council go
  through: an entry carrying `SHIELDED` is refused while
  `maya_zk_stark::pool::circuit_is_audited()` is false, and refused when the
  pool already has an activation (genesis or an earlier entry), so an
  on-chain vote can never produce the conflict that would stop
  `context_at`. Genesis entries are not gated on the audit: a testnet already
  runs the unaudited pool, and the node's startup guard refuses a
  value-bearing chain whose pool can run on it.
- **The node's startup guard reads the resolved height**, the genesis's own
  schedule included, before opening state, and again after the chain opens
  with the committed schedule.

## Alternatives rejected

- **Gate inside `UpgradeSchedule::new`**, as zkML is. That is also the read
  path: a test or a testnet could never build a schedule carrying the bit,
  and turning the audit flag on would be the only way to exercise the
  precedence rule.
- **Genesis always wins, silently.** An entry that names a height and does
  nothing is a vote whose result is ignored; refusing it is clearer.
- **Seeding the genesis height into state** so `admit` could read it. It
  adds a record to every genesis state root, and so changes every genesis.

## Consequences

- No genesis id changes: no genesis struct, commitment or seeded record
  changes, and `maya-mainnet` carries no `protocol_upgrades`
  (`genesis-check`). Pinned by `PINNED_BFT_EMPTY_LEVERS_ID` and checked
  against both tracked testnet genesis files.
- **Flipping `CIRCUIT_IS_AUDITED` is itself a consensus change.** Admission
  runs inside block execution, so a binary with the flag on accepts a
  `ScheduleUpgrade` carrying `SHIELDED` that a binary with it off refuses.
  The release that flips it must be adopted by the validator set before any
  such proposal is submitted, and should bump `SUPPORTED_PROTOCOL_VERSION`.
  Not enforced in code here.
- Turning the pool on stays two steps: an audit (the flag) and a vote under
  ADR-082's 30-day lead.

## Evidence

- `upgrade::tests::the_shielded_bit_is_known_and_resolves_from_its_entry`,
  `the_shielded_height_comes_from_exactly_one_source`
- `state::upgrades::tests::a_shielded_entry_is_refused_while_the_circuit_is_unaudited`,
  `a_shielded_entry_is_refused_when_the_pool_already_has_a_height`
- `genesis_tests::a_scheduled_shielded_pool_turns_on_exactly_at_its_height`,
  `a_genesis_naming_the_pool_twice_is_refused`
- `consensus_conformance` against CON-14 / CON-15 vectors from `crates/spec-ref`
