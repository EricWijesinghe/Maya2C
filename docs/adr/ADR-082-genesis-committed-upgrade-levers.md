# ADR-082: On-chain upgrade schedule with compiled leads

**Status:** Accepted, 2026-11-07 (Eric's decision, after the security review of the genesis-only design). Pre-genesis for the launch entries; the schedule itself is writable on chain after launch.
**Date:** 2026-11-07
**Amended:** 2026-11-15 (validity parameters through the schedule; 1 block/s constants).
**Related:** [ADR-037](ADR-037-mainnet-shielded-off.md), [ADR-013](ADR-013-suite-envelope-activation.md), [ADR-007](ADR-007-signature-suites.md), [ADR-050](ADR-050-owner-control-plane.md), invariants 12, 13, 17, 25, 34; `crates/node/src/upgrade.rs`, `crates/node/src/state/upgrades.rs`, `crates/governance/src/limits.rs`

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
- **zkML** never switches on until `zkml::verdict` is a real verifier (M3).
  Since the 2026-11-12 amendment it is not refused: the bit is outside
  `FeatureSet::IMPLEMENTED`, so an entry carrying it is unsupported and halts
  this binary at its height. A genesis that names it is still refused at boot.
- **Work claims** (`FeatureSet::WORK_CLAIM`) follow the zkML model while
  `WORK_CLAIM_ADR_ACCEPTED` is false: outside `FeatureSet::IMPLEMENTED`, so an
  entry is admitted and halts this binary at its height, and a genesis that
  names the bit is refused at boot (ADR-044 amendment 2026-11-15).
- **Feature bits** a binary does not define make the entry unsupported, not
  invalid (amendment, 2026-11-12).
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
its lead is `COUNCIL_SUNSET_LEAD_BLOCKS`, about 41.6 days at one block per
second (1,000,000 + 2,592,000 = 3,592,000 blocks).

**Why the council lead is what it is (H3).** A vault delay is at most
`vault::MAX_DELAY_BLOCKS` (1,000,000 blocks, about 11.6 days). A council sunset
announced just before a withdrawal could freeze the funds past the sunset height
unless the lead covers the delay. The migration window is the time users have to
move to another suite. The lead is the sum, derived from the constants. The
reviewer's figure of about 18.6 days assumed a 7-day migration window. The code's
`MIN_MIGRATION_WINDOW` is 2,592,000 blocks (30 days at 1 block/s), decided in
the re-freeze bundle (DECISIONS.md, 2026-11-13), which replaces the 2-day window
recorded on 2026-11-08.

**Update 2026-11-12 (block-time fix A).** These are new value decisions at the
canonical 1 block/s, not a unit fix: the earlier figures were already stated at
1 block/s, so the numbers themselves changed. The floors are now
`MIN_VOTING_BLOCKS` 7,200 (2 hours, was 480), `MIN_TIMELOCK_BLOCKS` 14,400
(4 hours, was 960), `MIN_MIGRATION_WINDOW` 2,592,000 (30 days, was 172,800) and
`MIN_EMERGENCY_WINDOW` 259,200 (3 days, was 17,280), so
`COUNCIL_SUNSET_LEAD_BLOCKS` is 3,592,000 blocks (about 41.6 days). The lead is
derived, so it moves with the window.

**No exit-only mode.** An exit a broken suite can still sign would let an attacker
exit a victim's funds too. Users migrate by transferring to an address under
another suite.

### Feature gates and the halt

A feature is on from the earliest entry at or below `SUPPORTED_PROTOCOL_VERSION`
that switches it on (`UpgradeSchedule::features`). An entry above it does not
enable anything here, and its height is the halt: a block at or past it is refused
with `UpgradeRequired`, so an old binary stops rather than validating under rules it
lacks. `SUPPORTED_PROTOCOL_VERSION` is 2, the first epoch with the gates.

### Amendment (2026-11-12): unknown feature bits halt, they do not refuse

The first version refused any entry with a bit outside `FeatureSet::KNOWN`, at
admission and when decoding a stored record. Every later upgrade adds bits, so
the genesis binary would refuse the governance block that schedules them: old
nodes reject that block at proposal or let the proposal expire at execution
while new nodes write the entry. That is a stall or a fork at the vote, months
before the activation.

Now validity does not depend on the binary, and support does. An entry is
**supported** by a binary only if its version is at most
`SUPPORTED_PROTOCOL_VERSION` *and* every bit is in `FeatureSet::IMPLEMENTED`
(`ProtocolUpgrade::is_supported_by`). `IMPLEMENTED` is `KNOWN` less zkML (no
verifier, M3) and less `WORK_CLAIM` while `WORK_CLAIM_ADR_ACCEPTED` is false
(ADR-044). An entry with any other bit is admitted under every other
rule (lead, horizon, ordering, sunsets), written, and decoded.
`first_unsupported` returns it, so `check_halt` refuses its activation height
with `UpgradeRequired` (the message names the bits) and the node stops there, as
for an unsupported version. Before that height nothing changes for the old
node, and the entry switches none of its features on in that binary.
`FeatureSet::is_known` stays the rule for which features a binary defines;
`IMPLEMENTED` for which it executes.

zkML moved from a refusal in `UpgradeSchedule::new` to "not implemented" for the
same reason. A refusal there made validity depend on the binary: a later binary
with a verifier would admit a zkML entry that this one refuses, a fork at the
vote. Now every binary admits it, and each one without a verifier halts at its
height, so unverified proofs still never run. A genesis is written for one
binary, so `GenesisConfig::upgrade_schedule` still refuses a bit outside
`IMPLEMENTED` at boot.

Governance gains no power: it could already schedule a version this binary does
not implement, which halts it at the same height. A sunset-only entry still
cannot switch any feature on, known or unknown, so the council's lever is
unchanged. Parameter tags (`ParameterKey::from_tag`) and suite bytes in a sunset
stay strict: a later binary may accept a new tag or suite only once its own
version has activated, by which height an older binary has already halted at
that version's entry. Accepting either earlier would stop older nodes at the
block carrying the proposal; that rule binds every future binary.

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

### Amendment 2026-11-15: block-validity parameters wait for the lead; 15 s-era constants at 1 block/s

Eric 2026-11-14 MCQ ("all format fixes"). Two changes, one reason: a rule that
decides which blocks are valid should never change faster than operators,
builders and wallets can follow, and a constant written as "a day" should be a
day at the rate the chain actually runs (docs/block-time.md).

**(a) Validity parameters through the schedule.** Before this, a passed change
to any `ParameterKey` was written into `g:params` at execution, so a per-block
ceiling could move one block after a timelock nobody outside governance was
watching. Now `ParameterKey::is_validity_rule` (an exhaustive match with no
wildcard: tags 2-8, 17 and 18, the DEX fill and order ceilings, the oracle and
shielded per-block caps, the three VM bounds and ADR-142's two gas caps) splits
the keys:

- a validity rule is written to `g:pending:<tag>` (under the governance layer, so
  under the state root and the undo journal, invariant 25) with activation
  height = execution height + `VALIDITY_RULE_LEAD_BLOCKS`;
- `VALIDITY_RULE_LEAD_BLOCKS` is `MIN_UPGRADE_LEAD_BLOCKS` (30 days), compiled
  (invariant 12), so no proposal shortens it, and a validity change carried
  beside a `ScheduleUpgrade` is in force no later than that entry's earliest
  activation;
- `StateDB::activate_parameters` runs at the end of every block after
  `settle_governance` and promotes each entry whose activation height is at
  most the next height, so the committed table a builder or mempool reads at
  the tip is the one the activation block is judged by (a builder never fills
  a block under a ceiling the importer has already moved). It re-checks the
  range at promotion; a value a later release no longer accepts is dropped,
  not applied and not a block error. It reads the nine validity tags by
  point lookup, not by a prefix scan, so its cost per block is fixed;
- the settle pass now also runs `ParameterChange::validate` before executing
  a proposal (it ran only `check_parameter` before), so a value a release
  tightened out of range while the vote was in flight expires the proposal
  instead of failing every candidate block at the execution height, which
  matches the promotion pass;
- at most one change waits per key: a later passed change to the same key
  replaces the one still waiting (the later decision is the current one, and
  the record stays bounded by the key count);
- the protocol fee, the proposal deposit and the default signature suite are
  not validity rules (a price, a deposit, a wallet default; invariant 31 keeps
  the default out of verification) and still apply at execution.

The proposal wire format is unchanged (invariant 31 is not touched): the lead
is fixed, not named by the proposer. Pinned by
`crates/node/tests/governance_tests.rs`
`a_block_validity_parameter_takes_effect_only_at_its_scheduled_height` (spec
CON-17), `a_later_passed_change_to_a_waiting_validity_rule_replaces_it`,
`a_replacement_executed_the_block_before_activation_keeps_the_old_value_out`,
`a_mixed_proposal_applies_its_price_at_once_and_waits_for_its_ceiling`,
`a_waiting_change_is_under_the_state_root_and_one_out_of_range_is_dropped`,
`a_passed_change_out_of_range_at_execution_expires_instead_of_halting`,
`a_proposal_of_only_validity_rules_leaves_the_table_unwritten_until_promotion` and `crates/governance/src/params.rs`
`only_the_rules_a_block_is_judged_by_wait_for_the_lead`. A new key added by a
later PR does not compile until it is classified. Open PR #549 (ADR-142) adds
tags 17 and 18 (`VmMaxTxGas`, `VmMaxBlockGas`): both are per-transaction and
per-block caps, so both are validity rules and take the lead. Whichever of the
two PRs merges second extends the test's tag list and the "tags 2-8" wording
here, in spec CON-17, `docs/governance.md`, `features.toml` and
`docs/architecture-vision.md`; classifying them as immediate just to compile
would let a gas cap move one block after a timelock. Done in #549, which
carries this amendment's PR (#575): tags 17 and 18 are validity rules, pinned
for the block cap by `a_passed_block_gas_cap_binds_only_after_the_validity_rule_lead`
(ADR-142 amendment 4).

Two limits, recorded rather than changed:

- **Visibility.** The waiting value is on chain under `g:pending:<tag>` from
  execution and readable by any state reader. No RPC lists it yet (the
  parameter RPC reads `g:params` only); a read-only view is a BACKLOG item
  after the freeze. Builders are unaffected: promotion lands at the tip they
  build on.
- **The vote lock stays at execution.** `lock_required_until` returns the
  execution height `E`, so a voter can unlock up to `MIN_UPGRADE_LEAD_BLOCKS`
  before the validity change it voted for binds. `ScheduleUpgrade` proposals
  already behave this way, and this amendment mirrors that path: the lock
  guards the decision against vote-then-sell, and the lead is notice to
  everyone else. Extending the lock by the lead would be a separate behaviour
  change with its own test.

**(b) Constants sized for 15 s (or 2 s) blocks, rescaled to 1 block/s.** The
remaining items of docs/block-time.md section 4 (items 1, 2 and 4 are in PR #500, open):

| Constant | Was | Now | Duration at 1 block/s |
|---|---|---|---|
| `governance::work::WORK_HALF_LIFE_BLOCKS` | 5,760 | 86,400 | one day |
| `crypto::dag::EPOCH_LENGTH` (and `DAG_ACTIVATION_HEIGHT`, defined from it) | 30,000 | 450,000 | 5.2 days |
| `smart_account::policy::BLOCKS_PER_DAY` (ADR-086 F7) | 43,200 | 86,400 | one day |
| `network::pq::ROTATION_INTERVAL_BLOCKS` (transport, not consensus) | 1,000 | 15,000 | 4.2 hours; `ROTATION_FALLBACK` stays 15,000 s |

Dependents checked: the mining epoch (`epoch_blocks` = 3,600) hashes against
the DAG epoch of its first block, and 450,000 is a whole 125 mining epochs;
committee rotation and staking read `epoch_blocks` from genesis, not the DAG
epoch; mining rewards read `halving_epochs`; the DAG vector file's
`mainnet_params.epoch_length` echoes the constant (the seeds and the hashes
are computed at the testing parameters and do not move). `TARGET_BLOCK_TIME`
keeps its value and is documented as the PoW retarget spacing only.

Not rescaled, on purpose: `state_pruner::PRUNE_DEPTH` was defined as
`EPOCH_LENGTH` and is now its own constant, kept at 30,000 (8.3 hours at one
block a second). It is node-local policy, not consensus, and DAG-BFT finality
means no honest reorg reaches it. Following the epoch to 450,000 would have
raised a default pruned node's worst case to about 2.1 TiB of bodies at the
5 MiB block cap (from about 146 GiB), and, because the default bootstrap and
snapshot-serve depths in `bins/maya2c-node/src/pruning.rs` are `PRUNE_DEPTH`,
no default snapshot would exist for the chain's first 5.2 days. The
`get_balance_at_height` RPC bound `MAX_BALANCE_REWIND` was also `PRUNE_DEPTH`;
it is now its own 30,000 too, so a public call walks at most 30,000 heights.
Both are pinned by unit tests that also assert they differ from `EPOCH_LENGTH`.

`MAX_LINKED_GAP` (`consensus/bft/catchup.rs`, 50,000 blocks, sized as 8.7 days
at 15 s) is a further 15 s-era constant docs/block-time.md did not list. It is
kept: at one block a second it is 13.9 hours, it is node-local (not
consensus), `MAX_LINKED_GAP_BYTES` (512 MiB) is the binding bound for
non-empty blocks, and a node offline longer restores with `--bootstrap-from`.
Its comment and docs/block-time.md now give the real duration.

Further dependents:

- **Not a rolling upgrade.** `dag_epoch()` (`state/mining_exec.rs`) divides a
  mining epoch's start by `EPOCH_LENGTH`, so from height 30,000 old and new
  binaries hash a mining share against different DAG caches. This binary
  needs the planned re-genesis and must not be rolled onto a running testnet.
- **Work-claim weight.** Decay is `credit >> elapsed / WORK_HALF_LIFE_BLOCKS`,
  so steady-state work credit, and its share of `eligible = locked + work`,
  grows with the half-life in blocks: about 15x here. Claims are refused at
  launch (`WorkClaimInactive`); whoever activates them must re-check the
  work-to-stake weight ratio at the new half-life first.
- **Duration tests are literal pins** (`86_400`, `450_000`, `15_000`). Once
  PR #500's `CANONICAL_ROUND_INTERVAL_MS` lands they should be derived from
  it, so a change to the canonical rate turns them red.

## Consequences

- **A schedule can be changed after launch**, which is the point. The genesis
  id does not change when governance or the council writes an entry.
- **Launch genesis ids change once** for proof-of-work genesis, because
  `pow_limit_bits` is folded in. The tree pins no PoW id. A pinned BFT id with
  empty levers is recorded in `genesis_tests.rs` (see below).
- **Leads are long.** A governance upgrade takes at least 30 days from
  execution to activation. A council sunset takes about 41.6 days (H3). A chain cannot be tested
  across those heights, so the on-chain halt is checked at the state level and
  the chain-level halt on genesis-seeded entries.
- **Mempool admission** and the stateless verifier use the effective schedule
  when the caller supplies it. The one-shot light-client path takes it from its
  caller (above).
- **Account migration for a sunset suite is not built.** It is a later
  activation (`BACKLOG.md`).

## Amendment 2026-11-13: the settlement pass reads only due proposals

**Problem.** `settle_governance`, the pass that executes a passed
`ScheduleUpgrade` (and every other proposal), read every `g:prop:` record on
every block: open, queued and settled alike. Settled records are never deleted,
the deposit returns on cancel, and nothing limits proposals per account, so an
attacker could grow that set for transaction fees alone, up to 32 per block
(`MAX_GOVERNANCE_TX_PER_BLOCK`). The x15 windows (PR #500) did not cause the
problem; they made each proposal live longer. Measured cost and the attack's
price: `reports/sessions/2026-11-13-governance-settle-index.md`.

**Decision.** An index keyed by the height a proposal next changes state,
under the governance layer that already exists:

- `g:due:<height u64 BE><proposal id>`, value empty. `propose` writes two keys:
  `voting_closes + 1` (finalize) and `executable_from` (execute, or expire if
  admission fails). The two keys cover every transition: `expires_after` is
  reached only by a proposal that sat `Queued` past `executable_from`, and that
  proposal's `executable_from` key is already due.
- The pass reads keys at or below its height (a bounded range read, so a
  replay that skips heights still finds them), deletes them, and settles the
  proposals they name in identifier order: the order the full scan used. The
  per-proposal step is the same function as before, and a proposal the index
  does not name was a no-op in the full scan. An equivalence test
  (`governance_settle_tests.rs`) checks the two paths agree on random sets.
- **At most `MAX_PROPOSALS_DUE_PER_HEIGHT` (64) keys per height.** Without it
  the index only moves the cost: the voting period ranges over 1.49 M values,
  so 32 proposals a block for 17 days could all come due at one height. A
  proposal that would overflow a height is refused, and its author picks a
  voting period or timelock one block different. An attacker cannot shut
  governance out this way: it would have to fill 64 slots at each of about
  1.49 M heights while adding 64 a block.
- **No new prefix.** `g:` is already the governance layer (`RECORD_LAYERS`), so
  the index is under the root, journaled by the overlay, and restored on
  reorg (invariant 25). It does change the governance layer's root whenever a
  proposal exists, which is why it lands before genesis (re-freeze).
- **No migration.** A proposal opened by an earlier binary has no index keys,
  and this pass would never settle it. A chain carrying one, such as the
  current testnet, re-geneses with the re-freeze bundle. It does not upgrade
  in place.

**Rejected.** A cap on concurrently open proposals: the deposit is refundable,
so an attacker holding the cap blocks every honest proposal for the price of
locked coin. A per-block budget with carry-over: it bounds a block too, but it
moves when a proposal finalizes, and an attacker who floods a height delays
everyone else's proposal behind it.

## Rejected alternatives

- **Genesis-only schedule** (the first draft). Rejected: the genesis id changes
  for any later entry, so the chain cannot follow it.
- **A protocol-wide config knob** for the lead. Rejected: a per-node setting would
  let two honest nodes disagree about when an entry takes effect. The leads are
  compiled into `limits.rs` like the other governance bounds (invariant 12).
- **One version per feature** (the rules epoch and the feature share a number).
  Rejected: it forces every dark feature to wait for the one before it.

## Amendment 2026-11-14: council hardening (founder audit Q7, Q8)

**Status:** Accepted (decision), 2026-11-14; implementation pending the merge
of PR #544 and green CI (`features.toml` status `working`). Eric chose the recommended option on every
founder-audit question ("I approve everything, proceed"): Q7 A plus a cooldown,
Q8 A. Pre-genesis: it changes the council action wire format and the council
record, so it ships in the re-freeze binary.

The audit found three gaps in the council this ADR made the fast path for
sunsets (`docs/invariants.md` invariant 34, `crates/node/src/state/council.rs`):

1. **The council could pause Governance.** `Module::Governance` covers stake,
   proposals and votes, and also mining shares and work claims
   (`Module::of`: `MiningShare` and `ClaimWork` pay out of the reserve, so
   ADR-044 put them under one breaker with staking). A council pause could
   freeze the vote that would answer the council. **Decision:** a council
   action that names Governance, pause or resume, is refused. Governance is
   never council-paused. The automatic breaker never named it either (its two
   checks gate the DEX and the shielded pool), so Governance is now ungated.
   *Deviation from the audit's wording:* option A said "remove Governance from
   `MODULES`". `MODULES` keeps it, because it lists every module tag and tags
   are permanent; the council action naming it is refused instead. The effect
   is the one option A asked for.
   *Trade-off, recorded in the audit:* the council can no longer slow an attack
   on governance itself, and it can no longer halt mining-share or work-claim
   payouts either. Those defences now rest on governance's own floors
   (invariant 12), the 30-day upgrade lead, and the work kinds' own checks.
2. **Renewals were unbounded.** A 2-of-3 quorum could renew a bounded pause
   back to back, so the bound meant nothing. **Decision:**
   `COUNCIL_RENEWAL_COOLDOWN_BLOCKS` (86,400 blocks, one day at one block per
   second; `crates/governance/src/limits.rs`). A council pause of a module is
   refused while the last council pause of that module holds, and for the
   cooldown after it ends. A council resume ends a pause, so the cooldown runs
   from the resume. A council sunset is refused within the cooldown of the
   council's last sunset (`last_sunset_at` in the record). The end of each
   module's last council pause is kept in the council record too
   (`last_pause_end`, by module tag). It is not read back from the breaker
   record, because an automatic trip (`trip_breaker`) overwrites that record:
   reading it there let an automatic trip during the cooldown clear the
   council's clock (review of PR #544, 2026-11-14).
   *Residual, from the audit:* nothing caps how many modules the council has
   paused at once. Each module has its own cooldown; the council can still
   pause every pausable module in one stretch.
   *Residual, review of PR #544 (2026-11-21):* a council pause or resume of a
   module writes that module's breaker record whatever it held, so the
   council can lift an automatic trip (resume) or replace it with a shorter
   council pause. A council resume always sets `last_pause_end` for the
   module, so lifting an automatic trip also starts the council's cooldown on
   that module. Both are kept: lifting a trip is what the council is for, and
   the cooldown only ever narrows what it may do next.
   *Ceiling on one pause (review of PR #544, 2026-11-17):* the cooldown bounds
   nothing if a genesis sets `max_pause_blocks` to years, and before this only
   0 was refused. `MAX_COUNCIL_PAUSE_BLOCKS` (86,400, equal to the cooldown, so
   a module is open at least as long as the council shut it) is compiled in
   `crates/governance/src/limits.rs`, and `CouncilGenesis::record` refuses 0 or
   anything above it. The recorded choice, six hours of blocks (DECISIONS.md,
   B30), is inside it; the ceremony rehearsal uses 21,600 and the proving run
   7,200.
3. **The council could not rotate.** If a key was lost or compromised, the
   only way to recover was re-genesis. **Decision:** a new action,
   `CouncilKind::Rotate` (wire kind 3). A quorum of the council *in force*
   signs a new member set and threshold under the rule genesis now applies
   too: 1 to `MAX_COUNCIL_MEMBERS` (32, one approval each must fit in
   `MAX_APPROVALS`) distinct, valid ML-DSA-65 keys, and a strict-majority
   threshold. *Genesis change:* before this amendment `CouncilGenesis::record`
   had no upper bound on members; it now refuses more than 32 (pinned by
   `a_genesis_council_of_more_than_32_members_is_refused`). A larger genesis
   council could never have acted, since a strict majority of 33 does not fit
   in 32 approvals. Every new member also signs an acceptance (domain
   `maya2c/security-council/rotation-accept/v1`, the chain tag and the
   rotation), carried in the action and checked when it lands. A key a typo or
   a hostile transaction builder put in the set has no holder to sign, so the
   rotation is refused rather than handing the council to nobody. The signing
   description shows each new key's 32-hex BLAKE3 prefix, the form the
   ceremony's commitment sheet prints, and `genesis-ceremony council accept`
   and `approve --rotate` build the action. The new set takes over at an activation height at least
   `COUNCIL_ROTATION_LEAD_BLOCKS` (86,400) after the block that carries it and
   no more than `MAX_SCHEDULE_HORIZON_BLOCKS` after it. Until then it sits in
   the council record as `pending`, which is in the state root, so the handover
   is public for the whole lead. A later rotation replaces a pending one, and
   it must clear the lead again from its own block. That is how a mistaken
   rotation is withdrawn. The handover advances the nonce by one, so no
   approval from before the handover is valid after it. An approval signs the
   nonce, not the member set, so without that step a key kept in the new set
   could have its approval, given under the old quorum and declined by the
   rest of it, counted toward the new threshold (review of PR #544,
   2026-11-21; pinned by
   `an_approval_given_before_a_rotation_does_not_count_after_it`). The
   cooldowns (`last_pause_end`, `last_sunset_at`) carry across the handover, so
   a new council cannot renew at once what the old one just did (pinned by
   `the_renewal_cooldowns_survive_a_rotation`). The handover is applied
   when the record is next read at or past the activation height:
   `CouncilRecord::at(height)` is a pure function of the record and the height,
   so every node agrees on the council in force at every height, and no
   per-block hook was added to the apply path. `StateDB::committed_council`
   takes a height and applies `at`, so a read after the activation shows the
   new set even before the council's next action writes the record back.
   *Deviation from the brief, which asked for the rotation to be "logged as an
   event":* this chain has no event log apart from the VM's. The rotation is
   recorded by the transaction that carries it and by the `pending` field in
   the record, which is in the state root. A separate event stream is not
   built.
   *Limit:* a rotation needs a live quorum of the council in force. If more
   keys are lost than the threshold allows (two of three, today), there is no
   on-chain recovery, and re-genesis remains the only path. Rotation recovers
   from a lost or compromised key while a quorum still holds.

**Record format.** The council record gains a version 2 that adds
`last_sunset_at`, `pending` and `last_pause_end` (module tags ascending,
each a known module). A council that has never acted still encodes as
version 1, byte for byte. Its genesis root is unchanged. Version 2 with every
added field empty, a stray sunset height, or pause ends out of order or for an
unknown module does not decode, so each value has one form.

**What this does not fix.** The council keys are still one operator's three
devices (DECISIONS.md 2026-10-18; audit Q9, the owner's decision). Rotation
lets those keys be replaced. It does not make the holders independent.

Pinned by `crates/node/tests/council_pause_tests.rs`:
`the_council_can_neither_pause_nor_resume_governance`,
`a_council_pause_cannot_be_renewed_within_the_cooldown`,
`a_council_pause_that_expires_unresumed_cools_down_from_its_end`,
`a_council_sunset_cannot_follow_another_within_the_cooldown`,
`a_rotation_with_too_few_approvals_is_refused`,
`a_rotation_must_clear_the_lead_and_name_a_sound_set`,
`a_rotation_past_the_schedule_horizon_is_refused`,
`a_rotation_naming_a_key_nobody_signed_for_is_refused`,
`a_later_rotation_replaces_a_pending_one_and_clears_the_lead_again`,
`a_rotation_takes_effect_at_its_height_and_not_before`,
`the_committed_council_read_after_activation_is_the_new_one`,
`an_approval_given_before_a_rotation_does_not_count_after_it`,
`the_renewal_cooldowns_survive_a_rotation`,
`a_genesis_council_of_more_than_32_members_is_refused`,
`a_genesis_council_pause_longer_than_the_compiled_ceiling_is_refused`; and by the
`state::council` unit tests `an_automatic_trip_does_not_reset_the_council_renewal_cooldown`
and `the_record_round_trips` (one form per record).
