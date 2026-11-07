# ADR-081: Genesis-committed upgrade levers

**Status:** Accepted, 2026-11-07. Pre-genesis: the genesis ids of every proof-of-work genesis change once, with this ADR.
**Date:** 2026-11-07
**Related:** [ADR-037](ADR-037-mainnet-shielded-off.md) (shielded activation in genesis), [ADR-013](ADR-013-suite-envelope-activation.md), [ADR-007](ADR-007-signature-suites.md), `crates/node/src/upgrade.rs`, `crates/node/tests/upgrade_rehearsal.rs`, invariants 24, 28, 29, 30, 31

## Context

An activation height that lives in a compiled constant is a fork, not a
decision. Two operators on the same genesis who run binaries with different
constants accept different blocks from the first height where they differ, and
nothing in the genesis id says so. An audit of `origin/master` found four
levers that become permanent at genesis and were not committed:

1. The dark-feature activation constants (`DVP`, `IoT`, threat intel,
   stateless, work claims, lattice HTLC, zkML, RWA strict issuers) are
   `u64::MAX` in `state::context` and `rwa_payload`. Only shielded had a
   genesis override.
2. `protocol_upgrades` was documented as "hashed into nothing", so two genesis
   files with different upgrade schedules had the same id.
3. The suite verification policy was `SuitePolicy::genesis(Mainnet)` with an
   empty deprecation map, so a broken suite could not be sunset by any
   upgrade.
4. `pow_limit_bits`, the retarget floor, was not in the id for proof-of-work
   genesis.

## Decision

**One lever for protocol change: the upgrade schedule.** Dark features are
activated only through `protocol_upgrades`, and the schedule is part of the
genesis id.

- Each dark feature has a protocol version (`upgrade.rs`): DVP 2, IoT 3,
  threat intel 4, stateless 5, work claim 6, lattice HTLC 7, zkML 8, RWA strict 9.
  `SUPPORTED_PROTOCOL_VERSION` is 9: this binary implements the activation
  mechanics for all of them.
- `UpgradeSchedule::features()` resolves the schedule to a `FeatureActivations`
  record of heights. A version above the binary's supported version contributes
  nothing. A `Copy` record is carried in `ChainConfig::features` and applied to
  every block context by `Chain::context_at`, through `BlockContext::with_features`.
- The compiled constants remain as the "never" defaults. A genesis with no
  schedule therefore executes exactly what it executed before.
- **Halt, not fork.** A binary below a scheduled version has
  `unsupported_upgrade` set to that entry. It refuses every block at or past
  the height with `UpgradeRequired`, as before. It does not validate under rules
  it lacks. Only binaries that implement the version can take the schedule's
  heights.
- `zkml::check_setup` (the public-seed SRS refusal) is checked against the
  height the schedule gives zkML, not the compiled default, so scheduling zkML
  cannot skip the check. The node binary runs it after `chain_config()`.
- **Commitment.** `GenesisConfig::chain_id_commitment` folds in a non-empty
  `protocol_upgrades` (sorted by height), a non-empty `suite_deprecations`
  (sorted by suite byte), and, for proof-of-work genesis only, `pow_limit_bits`.
  An empty upgrade schedule and no deprecations leave the id unchanged, apart
  from the `pow_limit` fold, which changes every PoW id once.

**Suite sunset.** `suite_deprecations` is a genesis record, empty at launch,
of `{suite, announced_at, sunset_height, emergency}`. It is validated against
`SuitePolicy` at genesis. The result is `SuiteSunsets`, a fixed-size `Copy`
record of at most one entry per registered suite, carried in
`ChainConfig::suite_sunsets` and `BlockContext::suite_sunsets`.
`suites::verification_policy(&SuiteSunsets)` derives the consensus policy from
it, and `state::db` verifies every block under the context's policy.
`check_admissible` then refuses a sunset suite at its sunset height and accepts
it one block earlier.

**Header version byte.** `HEADER_VERSION` in `core/block.rs` is a flag-day
tag, not negotiation: decode is strict (`core/block.rs:265-270`), and a header
whose version byte is not the one this binary speaks is refused. Changing it
means every node upgrades before the first block that carries the new tag, and
it is not governed by the upgrade schedule. Use it only for an encoding change
that cannot coexist with the old one.

## Consequences

- **Feature order is fixed by version number.** `UpgradeSchedule::new` requires
  versions and heights to increase together. Feature activations therefore
  happen in version order. To activate a lower-numbered feature after a
  higher-numbered one, the protocol needs a new version. This is a real limit,
  accepted for now because each feature is dark and the schedule is set once.
- **Mempool admission is not yet sunset-aware.** `network::mempool` admits
  with `crypto::suites::policy(parameters)`, which takes no genesis record.
  Admission is advisory, and `stage_transaction` at the block's height refuses
  a sunset suite, so no sunset suite reaches a block. Wiring the record into
  mempool admission is follow-up work, not a consensus gap.
- **Account migration is not built.** A sunset suite's accounts can still
  exist. `crates/crypto-pq/src/agility/migration.rs` is the place for
  migration. It is a later activation and is recorded in `BACKLOG.md`.
- **All proof-of-work genesis ids change once** (the `pow_limit` fold). The
  tree pins no PoW genesis id, so no vector needed updating.
- **Light-client shortcut.** `StatelessValidator` takes the schedule through
  `with_suite_sunsets` and defaults to none. `LightClient::verify_block` is a
  one-shot check and still judges under none. A chain that names a deprecation
  must pass it on every path. Fixing the one-shot path is follow-up work.
- **Mempool evidence admission** uses no schedule. The executor re-checks under
  the genesis schedule, so the mempool refusal is advisory.

## Rejected alternatives

- **Per-feature genesis height record** (one `u64` per feature, hashed when
  present, no version). It makes the height a genesis fact, but nothing stops an
  old binary from silently ignoring it. The halt rule would need a second
  mechanism, so this is two systems where the schedule is one.
- **Per-feature versions with independent heights** (a feature-indexed map that
  does not require monotonic heights). It removes the order limit, but it moves
  the halt rule off `UpgradeSchedule::new`, and the rehearsal and the
  "halt at H" guarantee are then no longer one code path.
- **Suite sunset in `SuiteId::info` or the registry.** The registry is a
  compile-time table and is in the crypto crate; a sunset is a per-chain fact.
