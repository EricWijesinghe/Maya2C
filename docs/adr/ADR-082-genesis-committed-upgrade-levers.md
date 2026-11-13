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
council, under the compiled leads.** Genesis may carry launch entries
(`protocol_upgrades`, `suite_deprecations`, and the PoW floor fold); the genesis
id covers them and genesis seeds them into state, so a later entry never touches
the id. `maya-mainnet` carries none: `genesis-check` enforces an empty
`protocol_upgrades` for it.

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

- **Versions** strictly increase in height among versions only
  (`UpgradeSchedule::new`), and **sunsets** strictly increase in announcement
  height among sunsets only. A sunset never fixes the height a version must
  follow, or the reverse (H1).
- **Horizon.** An activation or a sunset height may not pass the write height
  plus `MAX_SCHEDULE_HORIZON_BLOCKS` (about one year at one block per second),
  so one far entry cannot set a floor for every later one (H1).
- **Lead.** An entry is admitted only if its activation is at least the
  compiled lead after the height it is written at:
  `MIN_UPGRADE_LEAD_BLOCKS` (30 days at one block per second) for governance,
  `COUNCIL_SUNSET_LEAD_BLOCKS` for the council, derived as
  `vault::MAX_DELAY_BLOCKS + agility::MIN_MIGRATION_WINDOW` (H3). Governance
  checks at proposal time against the proposal's last possible execution height
  (`expires_after`), so every height the proposal can execute at keeps the full
  lead. It checks again at execution from the executing height.
- **Suites.** A sunset may not name the suite the network does not permit, nor
  the governance default suite at the moment of the write (H2). A default
  governance sets may not name a suite a schedule entry already retires, and a
  proposal whose own changes would break these rules expires instead of
  stalling the chain (H2).
- **zkML** cannot be scheduled until `zkml::verdict` is a real verifier (M3).
  The refusal is in `UpgradeSchedule::new`, so admission and genesis both apply it.
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
(ADR-050) and nonce apply unchanged. It has **no emergency window** (H3), and
its lead is `COUNCIL_SUNSET_LEAD_BLOCKS`, about 13.6 days at one block per
second (1,000,000 + 172,800 blocks).

**Why the council lead is what it is (H3).** A vault delay is at most
`vault::MAX_DELAY_BLOCKS` (1,000,000 blocks, about 11.6 days). A council sunset
announced just before a withdrawal could freeze the funds past the sunset height
unless the lead covers the delay. The migration window is the time users have to
move to another suite. The lead is the sum, derived from the constants. The
reviewer's figure of about 18.6 days assumes a 7-day migration window. The code's
`MIN_MIGRATION_WINDOW` is 172,800 blocks (2 days). The figure is recorded in
DECISIONS.md, and the decision on the window is open before mainnet.

**No exit-only mode.** An exit a broken suite can still sign would let an attacker
exit a victim's funds too. Users migrate by transferring to an address under
another suite.

### Feature gates and the halt

A feature is on from the earliest entry at or below `SUPPORTED_PROTOCOL_VERSION`
that switches it on (`UpgradeSchedule::features`). An entry above it does not
enable anything here, and its height is the halt: a block at or past it is refused
with `UpgradeRequired`, so an old binary stops rather than validating under rules it
lacks. `SUPPORTED_PROTOCOL_VERSION` is 2, the first epoch with the gates.

### What an old validator does at the halt (DAG-BFT)

The launch binary is the one that will be old at the first upgrade, so this
behaviour is fixed for good. Once its chain's next height is the halt height H,
the BFT driver (`consensus/bft/driver.rs`, `halt_reached`) stops feeding its
engine. It proposes no vertex, casts no vote and signs no attestation for any
height at or above H. It logs one line, `upgrade required before height H:
protocol version V activates there and this binary supports up to version S;
stopped proposing, voting and attesting`. After that, each tick and frame
returns at once with nothing to send.

The check runs before an engine output is persisted or sent. Whatever the
engine signed in memory in that step never leaves the process. DAG frames
already persisted in the step that built block H-1 are withdrawn too, while
their records stay in the safety log. An upgraded restart therefore never signs
those slots a second way. To its peers, the halted validator looks like a
crashed one. It leaves no equivocation evidence and no slashable message.

Votes that it cast before H-1 was built can certify vertices that upgraded
nodes later order into block H. A vote certifies that a vertex is available. It
says nothing about whether block H is valid, and upgraded nodes build block H
under their own rules. Pinned by `crates/node/tests/bft_node_tests.rs`
(`a_validator_without_the_scheduled_version_goes_silent_at_its_height_and_the_rest_carry_on`).

**Operator consequence.** Operators MUST upgrade before H. A halted validator
counts as a crashed one. With at most f such validators, the rest keep
finalizing. If more than f do not upgrade, finality stops at H for every node
until enough of them upgrade. The chain stalls and does not fork (pinned by
`more_than_f_validators_without_the_scheduled_version_stall_the_chain_without_a_fork`).
No test yet covers a validator that is upgraded after the halt and restarts
from its safety log, so how it rejoins is NOT VERIFIED.

Not changed here: a node restarted on the old binary with its tip already at
H-1 still refuses to start (`main.rs`, the start-up `check_halt`). It does not
come up and serve RPC for the history below H.

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

### Light clients (M2: a documented limitation, not a fix)

A light client has no state. `LightClient::verify_block` takes the suite
schedule from its caller and verifies with `verify_block_under`. A light client
therefore does not learn about on-chain sunsets itself: it trusts its caller for
them, and a stale schedule would accept a frame the chain has since refused.
The fix the review proposed, a witness that carries a state proof of the `up:`
records checked against the trusted root, is not built. The gap is recorded in
`BACKLOG.md`. Until then, a light client that follows a chain with a sunset must
be given the schedule from a source it trusts as much as its tip.

The public `state::verify_block` remains for the stateless tests. Its doc says
it judges under no schedule. Production verification uses `verify_block_under`.

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
  execution to activation. A council sunset takes about 13.6 days (H3). A chain cannot be tested
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

## Amendment 2026-11-14: council hardening (founder audit Q7, Q8)

**Status:** Accepted, 2026-11-14. Eric chose the recommended option on every
founder-audit question ("I approve everything, proceed"): Q7 A plus a cooldown,
Q8 A. Pre-genesis: it changes the council action wire format and the council
record, so it ships in the re-freeze binary.

The audit found three gaps in the council this ADR made the fast path for
sunsets (`docs/invariants.md` invariant 34, `crates/node/src/state/council.rs`):

1. **The council could pause Governance.** `Module::Governance` covers stake,
   proposals and votes, so a council pause could freeze the vote that would
   answer the council. **Decision:** a council action that names Governance,
   pause or resume, is refused. Governance is never council-paused. The
   automatic breaker never named it either (its two checks gate the DEX and the
   shielded pool), so Governance is now ungated. `MODULES` keeps Governance: it
   lists every module tag, and tags are permanent.
   *Trade-off, recorded in the audit:* the council can no longer slow an attack
   on governance itself. That defence now rests on governance's own floors
   (invariant 12) and the 30-day upgrade lead.
2. **Renewals were unbounded.** A 2-of-3 quorum could renew a bounded pause
   back to back, so the bound meant nothing. **Decision:**
   `COUNCIL_RENEWAL_COOLDOWN_BLOCKS` (86,400 blocks, one day at one block per
   second; `crates/governance/src/limits.rs`). A council pause of a module is
   refused while the last council pause of that module holds, and for the
   cooldown after it ends. A council resume ends a pause, so the cooldown runs
   from the resume. A council sunset is refused within the cooldown of the
   council's last sunset (`last_sunset_at` in the record). The breaker record
   already carries the pause's end, so the pause rule needs no new state.
3. **The council could not rotate.** If a key was lost or compromised, the
   only way to recover was re-genesis. **Decision:** a new action,
   `CouncilKind::Rotate` (wire kind 3). A quorum of the council *in force*
   signs a new member set and threshold under the same rules genesis applies:
   1 to `MAX_COUNCIL_MEMBERS` (32, one approval each must fit in
   `MAX_APPROVALS`) distinct, valid ML-DSA-65 keys, and a strict-majority
   threshold. The new set takes over at an activation height at least
   `COUNCIL_ROTATION_LEAD_BLOCKS` (86,400) after the block that carries it and
   no more than `MAX_SCHEDULE_HORIZON_BLOCKS` after it. Until then it sits in
   the council record as `pending`, which is in the state root, so the handover
   is public for the whole lead. A later rotation replaces a pending one, and
   it must clear the lead again from its own block. That is how a mistaken
   rotation is withdrawn. The nonce carries on across the handover, so no
   approval from before the handover is valid after it. The handover is applied
   when the record is next read at or past the activation height:
   `CouncilRecord::at(height)` is a pure function of the record and the height,
   so every node agrees on the council in force at every height, and no
   per-block hook was added to the apply path. *Event:* this chain has no
   event log apart from the VM's. The rotation is recorded by the transaction
   that carries it and by the `pending` field in the record (readable through
   `StateDB::committed_council`). A separate event stream is not built.

**Record format.** The council record gains a version 2 that adds
`last_sunset_at` and `pending`. A council that has never sunset or rotated
still encodes as version 1, byte for byte. Its genesis root is unchanged.
Version 2 with both fields empty does not decode, so each value has one form.

**What this does not fix.** The council keys are still one operator's three
devices (DECISIONS.md 2026-10-18; audit Q9, the owner's decision). Rotation
lets those keys be replaced. It does not make the holders independent.

Pinned by `crates/node/tests/council_pause_tests.rs`:
`the_council_can_neither_pause_nor_resume_governance`,
`a_council_pause_cannot_be_renewed_within_the_cooldown`,
`a_council_sunset_cannot_follow_another_within_the_cooldown`,
`a_rotation_with_too_few_approvals_is_refused`,
`a_rotation_must_clear_the_lead_and_name_a_sound_set`,
`a_rotation_takes_effect_at_its_height_and_not_before`.
