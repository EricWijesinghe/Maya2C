# ADR-087: Enable the shielded pool after genesis through a scheduled upgrade

- Status: **Proposed (2026-11-10).** Not accepted. Needs Eric's decisions listed below,
  and the audit gate in decision 5 stays closed until an independent review exists.
- Scope: consensus (activation rules), state (a new value-moving feature), governance
  (upgrade writer). Changes what a block means from the activation height on.
- Number claim: `087 docs/shielded-pool-plan` in the coordinator registry (not in this repo).
- Research and facts: [docs/research/shielded-pool-activation-2026-11-10.md](../research/shielded-pool-activation-2026-11-10.md).
- Related: [ADR-037](ADR-037-mainnet-shielded-off.md) (pool off for mainnet v1),
  [ADR-067](ADR-067-shielded-and-private-amounts-256-bit.md) (widths),
  [ADR-068](ADR-068-forever-mining.md) (24 decimals), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md)
  (upgrade schedule), invariants 24, 25, 28, 34.

## Context

ADR-037 keeps the shielded pool off on mainnet v1. `CIRCUIT_IS_AUDITED = false`
(`crates/zk-stark/src/pool/mod.rs:36`) and `bins/maya2c-node/src/main.rs:117-135` refuses a
value-bearing network that enables the pool with an unaudited circuit. Today the pool can
only be enabled at genesis: `shielded_activation` comes from `GenesisConfig`
(`crates/node/src/genesis.rs`), and `FeatureSet` has no shielded bit
(`crates/node/src/upgrade.rs`, `KNOWN = (1 << 8) - 1`). A chain that launched with the pool
off cannot turn it on later, even after an audit.

## Decision

1. **Feature bit.** `FeatureSet::SHIELDED = 1 << 8`, appended. `KNOWN` becomes `(1 << 9) - 1`.
   Bit 9 stays unused here: it is reserved for `ACCOUNT_AUTHORITY` (wallet-v2, ADR-086).
2. **Activation field.** `FeatureActivations` gains `shielded: u64`, filled by
   `UpgradeSchedule::features()` from the earliest entry at or below the supported version
   that sets the bit.
3. **Version.** `SUPPORTED_PROTOCOL_VERSION` moves from 2 to 3 only in the audited binary.
   A binary still at 2 halts at the entry's height instead of guessing (existing behaviour).
4. **Precedence.** Exactly one source may set the height:
   - genesis sets a height and the schedule has no SHIELDED entry: genesis wins;
   - genesis is `SHIELDED_NEVER` and the schedule has a SHIELDED entry: the schedule wins;
   - both set: the node refuses to start.
   The genesis commitment (`genesis.rs` `shielded_commitment`) is unchanged when genesis
   does not set a height, so the mainnet genesis id stays pinned.
5. **Audit gate.** The schedule writer refuses a SHIELDED entry while
   `circuit_is_audited()` is false. The `main.rs` guard reads the effective activation
   (genesis or schedule) instead of genesis alone. `CIRCUIT_IS_AUDITED = true` goes in a
   separate, reviewed commit that cites the audit record, never in the feature commit.
6. **Lead time.** Governance writes `up:v3` with at least `MIN_UPGRADE_LEAD_BLOCKS`
   (`crates/governance/src/limits.rs:90`, 2,592,000 blocks, about 30 days at one block per
   second) between execution and activation.
7. **Halt.** Validators still on version 2 halt at the activation height.
8. **ADR-082 amendment (to be written with the accepted decision).** Replace "no funds, no
   minting" for shielded transfers with: a value-moving feature needs an audit record in the
   repository and `circuit_is_audited() == true`, enforced in the writer.

## Not decided here (Eric)

- Cap width: keep 2^54 for a capped phase, or widen before the audit (ADR-067).
- Pool total and per-transaction caps: MISSING.
- Sender hiding: the envelope signs with a public account and nonce today.
- Note delivery: no ciphertext in the payload today.
- Auditor and cost: after mainnet, per the owner's directive.
- Supply integrity: the turnstile tracks public flows only, so a minting bug inside the pool
  is bounded only by the drain breaker (`anomaly.rs:181-200`). A sum-of-unspent-values check
  is needed. Any new invariant number is assigned only once a test pins it.

## Consequences

- Enabling the pool becomes a governed upgrade, not a genesis-only choice, and the audit gate
  is enforced by code rather than by convention.
- Mainnet genesis id is unchanged by this decision (pin test required in the implementation).
- The implementation (`postgen/shielded-feature-bit`) is post-genesis and does not run in the
  frozen node binary.
