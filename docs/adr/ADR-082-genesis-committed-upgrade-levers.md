# ADR-082: On-chain upgrade schedule with compiled leads

**Status:** Accepted, 2026-11-07 (Eric's decision, after the security review of the genesis-only design). Pre-genesis for the launch entries; the schedule itself is writable on chain after launch.
**Date:** 2026-11-07
**Related:** [ADR-037](ADR-037-mainnet-shielded-off.md), [ADR-013](ADR-013-suite-envelope-activation.md), [ADR-007](ADR-007-signature-suites.md), [ADR-050](ADR-050-security-council.md), invariants 12, 13, 17, 25, 34; `crates/node/src/upgrade.rs`, `crates/node/src/state/upgrades.rs`, `crates/governance/src/limits.rs`

## Context

Four levers were permanent at genesis and not committed: dark-feature
activation heights were compiled constants, `protocol_upgrades` was hashed
into nothing, the suite policy had no deprecation schedule, and `pow_limit_bits`
sat outside the id.

The first fix committed the launch schedule into the genesis id. A security
review then showed the flaw: an entry added after launch changes the genesis id,
so every node built from the old genesis refuses the chain (`GenesisMismatch`),
the chain tag changes, and every signature becomes invalid. A suite sunset
committed that way cannot be used after launch at all.

## Decision

**The schedule is committed state, written by governance or the security
council, under the compiled leads.** The genesis id keeps the launch entries
(`protocol_upgrades`, `suite_deprecations`, and the PoW floor fold). Genesis
seeds those entries into state, so a later entry never touches the id.

### What the state holds

Records under the new prefix `up:` (`state::upgrades`), a committed layer
`StateLayer::UpgradeSchedule` (tag 17), so the state root covers it (invariant 25):

- `up:v` + version (big-endian `u32`) → a version entry: rules epoch,
  activation height, and the `FeatureSet` it switches on;
- `up:s` + suite byte → a sunset: announcement height, sunset height, emergency flag.

`EffectiveSchedule` is read from those records. It is the one source for the
block context (`Chain::context_at`), the halt check (on import and at start-up),
the suite policy, the mempool, and the RPC.

### Rules every entry obeys

- **Versions** strictly increase, and **heights** strictly increase
  (`UpgradeSchedule::new`). Every new entry must lie after every existing one.
- **Lead.** An entry is admitted only if its activation is at least the
  compiled lead after the height it is written at:
  `MIN_UPGRADE_LEAD_BLOCKS` (30 days at one block per second) for governance,
  `MIN_COUNCIL_SUNSET_LEAD_BLOCKS` (7 days) for the council. Governance checks at
  proposal time against the proposal's last possible execution height
  (`expires_after`), so every height the proposal can execute at keeps the full
  lead. It checks again at execution from the executing height.
- **Feature bits** must be defined (`FeatureSet::is_known`).
- **Nothing is cancelled or moved.** A record is written once. There is no
  removal path.
- **No funds, no minting.** An entry switches a dark feature on or retires a
  suite. It writes no balance.

### Writers

**Governance: `ScheduleUpgrade`.** A `ProposalSubmission` may carry an
`upgrade`: `{ version, activation_height, features, sunsets }`. It travels as a
trailing section, so a parameter-only proposal keeps its shape. The proposal
passes or fails by the ordinary vote and timelock (invariants 12 and 13). On
execution the entry is admitted and staged in the same block. An entry that
has lost its lead is not applied and the proposal expires. Refusing the block
would stop the chain over a vote that passed.

**Security council: `SunsetSuites`.** A new `CouncilKind` (tag 2) carrying only
`announced_at` and suite sunsets. It has no version and no features, so the
council can retire a suite and schedule nothing else. Its own 2-of-3 approvals
(ADR-050) and nonce apply unchanged.

### Feature gates and the halt

A feature is on from the earliest entry at or below `SUPPORTED_PROTOCOL_VERSION`
that switches it on (`UpgradeSchedule::features`). An entry above it does not
enable anything here, and its height is the halt: a block at or past it is refused
with `UpgradeRequired`, so an old binary stops rather than validating under rules it
lacks. `SUPPORTED_PROTOCOL_VERSION` is 2, the first epoch with the gates.

### Rules epoch separate from features (HIGH-1)

The version is the rules epoch and only increases. Each entry carries the
`FeatureSet` it switches on, so any dark feature can join any later version.
A feature does not have to wait for the one numbered after it. `version_at`
still reports the epoch in force.

### Suite policy

The consensus policy is `verification_policy(&SuiteSunsets)`, built from the
effective sunset records. `check_admissible` refuses a sunset suite at its
sunset height and accepts it one block earlier. The mempool, the stateless
verifier and threat evidence read the same records.

### Light clients

A light client has no state. `LightClient::verify_block` takes the suite
schedule from its caller and verifies with `verify_block_under`. The caller
still has to be trusted for the schedule. That gap is recorded in `BACKLOG.md`.

### Header version byte

`HEADER_VERSION` (`core/block.rs`) is a flag-day tag, not negotiation. Decode is
strict (`core/block.rs:265-270`). A header whose version byte this binary does
not speak is refused. Changing it means every node upgrades before the first
block that carries the new tag, and the upgrade schedule does not govern it.

## Consequences

- **A schedule can be changed after launch**, which is the point. The genesis
  id does not change when governance or the council writes an entry.
- **Launch genesis ids change once** for proof-of-work genesis, because
  `pow_limit_bits` is folded in. The tree pins no PoW id. A pinned BFT id with
  empty levers is recorded in `genesis_tests.rs` (see below).
- **Leads are long.** A governance upgrade takes at least 30 days from
  execution to activation. A council sunset takes 7. A chain cannot be tested
  across those heights, so the on-chain halt is checked at the state level and
  the chain-level halt on genesis-seeded entries.
- **Mempool admission** and the stateless verifier use the effective schedule
  when the caller supplies it. The one-shot light-client path takes it from its
  caller (above).
- **Account migration for a sunset suite is not built.** It is a later
  activation (`BACKLOG.md`).

## Rejected alternatives

- **Genesis-only schedule** (the first draft). Rejected: the genesis id changes
  for any later entry, so the chain cannot follow it.
- **A protocol-wide config knob** for the lead. Rejected: a per-node setting would
  let two honest nodes disagree about when an entry takes effect. The leads are
  compiled into `limits.rs` like the other governance bounds (invariant 12).
- **One version per feature** (the rules epoch and the feature share a number).
  Rejected: it forces every dark feature to wait for the one before it.
