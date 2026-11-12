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

## Amendment 2026-11-12: admission does not read the audit flag

Review of PR #420 found that `check_shielded_entry` read
`circuit_is_audited()`, a compile-time constant, inside block execution. Two
binaries built with the flag on and off would then disagree about whether
the `ScheduleUpgrade` carrying `SHIELDED` is valid, and the chain would split
at the vote block, not at the activation height. The "Consequences" note
above described this and left it unenforced.

Changed:

- **Admission (CON-15) is flag-free.** An on-chain entry carrying `SHIELDED`
  is admitted when it names a protocol version of at least
  `SHIELDED_MIN_PROTOCOL_VERSION` (3, a literal: above every binary shipped
  so far, which all support version 2), and the pool has no activation yet.
  The pool check counts every entry in the schedule, whatever its version,
  so it does not depend on the binary's `SUPPORTED_PROTOCOL_VERSION` either.
- **The audit flag gates execution only, through the halt.**
  `UpgradeSchedule::first_unsupported` treats an entry at or above
  `SHIELDED_MIN_PROTOCOL_VERSION` that carries `SHIELDED` as unsupported on a
  binary whose circuit is not audited. Such a binary stops with
  `NodeError::UpgradeRequired` at the activation height (the ADR-082
  protocol-upgrade halt) instead of executing or refusing join-splits. A
  binary older than version 3 halts there anyway.
- Genesis entries below version 3 are unchanged: no audit clause, and the
  startup guard still refuses a value-bearing chain whose pool can run.

Evidence: `state::upgrades::tests::audited_and_unaudited_binaries_agree_on_a_shielded_entry`,
`a_shielded_entry_must_name_a_version_above_every_older_binary`;
`upgrade::tests::an_unaudited_binary_halts_at_a_shielded_entry`; CON-15
vectors regenerated.
