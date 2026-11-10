# ADR-095: v2 network upgrade (umbrella)

**Status:** Proposed. Post-genesis. Nothing here changes the frozen node binary (bb4e0940) until genesis; each item lands as its own draft PR and merges after Eric's APPROVED.
**Date:** 2026-11-11
**Related:** [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), [ADR-081](ADR-081-address-text-bech32m.md), [ADR-086](ADR-086-wallet-v2-seedless-recovery-and-clear-signing.md), ADR-087 as written in PR #420 (to be renumbered ADR-093; ADR-087 itself stays with `docs/shielded-pool-plan`), invariants 12, 24, 32, 33, 34, 35 (proposed in PR #447); `crates/node/src/upgrade.rs`, `crates/node/src/state/upgrades.rs`, `crates/governance/src/limits.rs`, `crates/fee-market/src/limits.rs`

## Context

Genesis is frozen. Everything in the post-genesis set has to ship as a rules-epoch change that an activation height switches on, because a binary that is already running cannot be edited. ADR-082 gives the mechanism: the schedule (`up:` entries) names a version, an activation height and dark feature bits, and a binary that does not implement a scheduled version halts at that height instead of validating under old rules (`crates/node/src/upgrade.rs:15-17`).

Four draft PRs and one shipped fix are the v2 set. They touch the same three files (`upgrade.rs`, `docs/adr/README.md`, `docs/invariants.md`), so they need one umbrella that says what each one is, in what order they may land, and what the activation rule is. This ADR is that umbrella. It does not decide any single item; each item keeps its own ADR or PR.

Current state on `postgen/v2` (verified in code):

| Fact | Where |
|---|---|
| `SUPPORTED_PROTOCOL_VERSION = 2` | `crates/node/src/upgrade.rs:34` |
| `FeatureSet` defines bits 0–7; `KNOWN = (1 << 8) - 1` | `crates/node/src/upgrade.rs:40`, `:60` |
| Lead constant `MIN_UPGRADE_LEAD_BLOCKS = 2_592_000` | `crates/governance/src/limits.rs:90` |
| Lead check at proposal `admit(...)`, same constant | `crates/node/src/state/governance_exec.rs:440-450`, `crates/node/src/state/upgrades.rs:138` |
| Halt check `check_halt` | `crates/node/src/state/upgrades.rs:123` |
| Version reported by RPC `get_node_version` (`protocol` field) | `crates/node/src/rpc/server.rs:526-527` |
| `CIRCUIT_IS_AUDITED = false` | `crates/zk-stark/src/pool/mod.rs:36` (postgen/v2) |
| `MAX_BLOCK_BYTES = 5 MiB` (invariant 33) | `crates/fee-market/src/limits.rs:60` |
| `MAX_TX_BYTES = 4 MiB` | `crates/node/src/core/transaction.rs:127` |
| `HEADER_LEN = 145`, block overhead = header + 8-byte count | `crates/node/src/core/block.rs:174`, `:14-18` |

## Decision

Eight items. Each row says what it is, its status today, and what it needs.

| # | Item | Vehicle | Status today | Needs before merge |
|---|---|---|---|---|
| A1 | **Supply: non-circulating reserve.** The mining reserve holds the lifetime emission and is not spendable by any key. | Genesis and state accounting | Reserve exists: `MINING_RESERVE` (`crates/node/src/state/mining.rs:41`), genesis must fund it with the lifetime emission (`crates/node/src/genesis.rs:996-1020`). **No "circulating supply" value exists in code** (grep for `circulat` in `crates/node` and `crates/governance`: no hits). | Eric decides what counts as circulating (open question 1). Then a read-only RPC or test. No consensus change if it is a view alone. |
| A2 | **Propose describes the upgrade.** A governance proposal carries the `ScheduleUpgrade` (version, height, features, sunsets) and the schedule is checked at submission with the compiled lead. | Governance payload | Exists: `ScheduleUpgrade` (`crates/node/src/core/governance_payload.rs:60-69`), proposal field `upgrade` (`:156`), refused at submission if the lead is short (`governance_exec.rs:440-450`), re-checked at execution (`:616`). | Tests in `upgrade_schedule_tests.rs` already pin the lead. Add: a proposal that names bit 9 is refused until every validator's binary knows bit 9 (see A5). |
| A3 | **Mempool and block build agree.** A transaction the mempool admits must not be silently dropped at block build. | Mempool and block selection | Gap confirmed by PR #439 (test-only): `select_applicable_within` maps every error to `false` with the reason discarded (`crates/node/src/state/db.rs:1149`). `Mempool::validate` (`crates/node/src/network/mempool.rs:241`) does not run payload execution rules. | Fix belongs in #439's follow-up: log the reason, add a status query, evict after repeated drops. Node code, so post-genesis. |
| A4 | **Bech32m node.** Genesis and RPC accept and return `maya1…` / `tmaya1…` text beside hex. | Genesis decode, RPC | PR #434 (draft, base `master`). Hex is never removed. Genesis identity unchanged by its own tests (per PR body; not re-run here). | Rebase onto `postgen/v2` (see Task 3 table). ADR-081 is the decision record. |
| A5 | **Account authority, bit 9.** Wire v9 account-bound frames, account records, recovery. Dark until a schedule sets bit 9. | Wire, state, feature bit | PR #447 (draft, base `master`). Sets `ACCOUNT_AUTHORITY = 1 << 9` and `SUPPORTED_PROTOCOL_VERSION` 2 → 3 (per its body). Proposes invariant 35. Size against invariant 33: §Byte cap below. | Rebase; resolve bit-8 ordering with A6. Frame sizes measured in the branch, not on `postgen/v2`. |
| A6 | **Shielded pool, bit 8, code solely.** The schedule can carry bit 8, and the writer refuses it while the circuit is unaudited. | Feature bit, schedule writer | PR #420 (draft, base `master`), ADR to be renumbered to ADR-093. `CIRCUIT_IS_AUDITED = false` (`crates/zk-stark/src/pool/mod.rs:36`). Its own self-review (HIGH 1) says flipping the flag is a consensus change. | **`CIRCUIT_IS_AUDITED` stays `false` in this umbrella.** Flipping it is a separate consensus change with its own audit, its own version bump, and its own ADR. Not part of v2. |
| A7 | **Exit-window derivation (ADR-096).** `MIN_UPGRADE_LEAD` must be at least vault delay + unlock + unbonding, so a user can exit before a scheduled change takes effect. | Governance limits | Vault delay ceiling `MAX_DELAY_BLOCKS = 1_000_000` (`crates/node/src/state/vault.rs:31`). Unbonding window: `unbonding_epochs` exists (`crates/node/src/consensus/bft/evidence.rs:7`), value not yet read. Lead today: 2,592,000 blocks (`governance/src/limits.rs:90`). | ADR-096 derives the sum. **MISSING: the unbonding value in blocks** (needs the genesis value and epoch length). Also see the block-cadence finding below. |
| A8 | **Upgrade-readiness advertisement.** Operators can see that a binary is behind the schedule before the halt. | RPC | Partly exists: `get_node_version` returns `protocol: SUPPORTED_PROTOCOL_VERSION` and the crate version (`crates/node/src/rpc/server.rs:522-528`). It does not report the scheduled next version or its activation height. | Add to the RPC response: `scheduled_next_version`, `scheduled_activation_height`, read from `UpgradeSchedule::entries()`. Read-only, additive. |

### Activation rule

1. A `ScheduleUpgrade` for v3 (or any later version) takes effect no sooner than **genesis + 3 months** (the W1 stability window, as set in the brief). **No code enforces a genesis-relative floor today.** The compiled lead (`MIN_UPGRADE_LEAD_BLOCKS`) is measured from execution, not from genesis. The floor is a new check and needs a new constant in `crates/governance/src/limits.rs`. Its value in blocks depends on the block cadence (see the finding below).
2. After execution, the activation height is at least `MIN_UPGRADE_LEAD_BLOCKS` past the execution height (`crates/governance/src/limits.rs:98-100`, `is_upgrade_lead_valid`). This already holds in code.
3. `SUPPORTED_PROTOCOL_VERSION` is bumped in the release that carries any v3 feature (A5 gives 2 → 3). It is bumped by the same binary that implements the features; never earlier.

**Finding that changes the numbers (block cadence).** The lead and horizon constants are documented as "30 days at one block per second" (`crates/governance/src/limits.rs:85`, `docs/invariants.md:257`). The code does not set one block per second:

- The BFT round interval defaults to **500 ms** (`crates/node/src/genesis.rs:420-422`, `default_round_interval_ms`). The testnet genesis file does not set it (default applies).
- `TARGET_BLOCK_TIME = 15` s (`crates/node/src/consensus/difficulty.rs:32`) is the proof-of-work retarget target, not the BFT cadence.
- **MISSING: measured blocks per second on the running chain.** Until measured, "30 days" in blocks is unknown. If one block is produced per 500 ms round, 2,592,000 blocks is about 15 days, and 3 months is 15,552,000 blocks, not 7,776,000. The constants must be re-derived from the measured cadence before v2 activates. This is an input to the activation rule, not a decision made here.

Computed (not measured): 3 months = 7,776,000 blocks at 1 block/s; 15,552,000 blocks at 2 block/s.

### SUPPORTED_PROTOCOL_VERSION

Stays 2 on `postgen/v2`. #447 moves it to 3. #420 may also bump it; if both do, the bumps are ordered (one 3, one 4), and the merged value is the highest in use. A6 does not bump it (see A6).

## Byte cap (invariant 33): A5 frames per block

`MAX_BLOCK_BYTES = 5 * 1024 * 1024 = 5,242,880` bytes (`crates/fee-market/src/limits.rs:60`). Each transaction costs its encoded length plus an 8-byte length prefix. The block overhead is `HEADER_LEN + 8 = 153` bytes (`crates/node/src/core/block.rs:14-18`, `:174`). The block selector admits a transaction solely while the running total stays at or under the cap (`crates/node/src/core/block.rs:41`, `:44-53`).

Frame sizes are from PR #447's measurement, `reports/bench/account-authority-v9-frame-bytes.md` on branch `postgen/account-authority`, run as `cargo test -p custom-l1-node --test account_authority_tests measured_v9_frame_bytes -- --nocapture`. Re-checked in the report text; the numbers agree with the PR body (28,420 and 43,560). Measured on base `027c7813`, not on `postgen/v2`.

| Frame (one output, suite 0x30) | Encoded | Wire (+8) | Frames in an otherwise empty block | Bytes left over |
|---|---|---|---|---|
| 2-of-3 | 28,420 | 28,428 | **184** (184 × 28,428 = 5,230,752 + 153 ≤ cap) | 11,975 |
| 3-of-5 | 43,560 | 43,568 | **120** (120 × 43,568 = 5,228,160 + 153 ≤ cap) | 14,567 |

Reading: 184 and 120 are the ceilings for a block made solely of these frames. A real block also carries other transactions, so the realistic count is lower. The naive `5 MiB / frame` gives the same integers. No size target is asserted anywhere. This is computed from measured sizes, so it is not an invented number. Not verified: a full block built and executed with 184 such frames (no test in #447 does this).

Consequence for A5: at 184 frames, a 2-of-3 account authority mass-migration costs 184 transactions per block. Fee pricing already follows bytes (`MAX_TARGET_BLOCK_BYTES`, `crates/fee-market/src/limits.rs:49`). No change to the cap is proposed.

## Rebase and base-branch status (Task 3)

All four PRs are opened against `master`. None has been retargeted. Eric or the coordinator must run `gh pr edit <n> --base postgen/v2` once this ADR's PR is in. Trial rebases were run in scratch branches on this worktree and aborted.

| PR | Head | Base today | Replays onto `postgen/v2` | Notes |
|---|---|---|---|---|
| #420 | `postgen/shielded-feature-bit` | master | **Conflict** in `docs/adr/README.md` (rc 1) | Also conflicts with #447 (see below). ADR renumber 087 → 093 needed. |
| #434 | `postgen/node-address-text` | master | Clean (rc 0) | Needs `gh pr edit --base postgen/v2`. |
| #439 | `test/did-silent-drop` | master | Clean (rc 0) | Needs `gh pr edit --base postgen/v2`. Test-only; no node code changed per its body. |
| #447 | `postgen/account-authority` | master | Clean (rc 0) | Needs `gh pr edit --base postgen/v2`. |

Cross-PR conflicts (checked with `git merge-tree --write-tree`, no checkout):

- #420 vs #447: **conflicts** in `crates/node/src/upgrade.rs`, `docs/adr/README.md`, `docs/invariants.md`. Expected: both claim feature bits and the `KNOWN` mask. Merge order: whichever lands second resolves `KNOWN` (#447 says `(1 << 10) - 1` once both land) and the version bumps.
- #434 vs #447: clean.

Merge order proposed: #434 and #439 (no feature bits), then #447 (bit 9, version 3), then #420 (bit 8, after resolving the mask). A6 stays code-only, so #420 can merge with the audit flag still `false`.

## Test plan

1. **Proving run on the RC.** 24 h proving run on the release candidate built with `cargo xtask release-check` (`xtask/src/main.rs:77`). Per Eric's 2026-11-02 directive, no multi-day soak: 24 h maximum.
2. **Attacknet across the activation height.** `cargo xtask attacknet` (`xtask/src/attacknet.rs:897`) with a schedule entry at height H. Validators that run the v3 binary cross H and keep producing. Validators that run v2 **stall at H and do not fork**. Pass criterion: no two chains finalize different blocks at or after H, and the stalled set reports "upgrade required before height H" (`crates/node/src/error.rs:148`).
3. **Lead and floor tests.** A schedule one block short of `MIN_UPGRADE_LEAD_BLOCKS` is refused at proposal and at execution (existing: `upgrade_schedule_tests.rs`). New: a schedule below genesis + 3 months is refused (A-activation rule 1, once the constant exists).
4. **Feature bit tests.** Bit 8 and bit 9 are refused as unknown on a binary whose `KNOWN` does not include them (existing pattern: `upgrade.rs` tests around `KNOWN + 1`).
5. **Byte cap.** A block of 184 2-of-3 frames is built and executed on one node, and a 185th is refused or deferred. Not written yet (see Byte cap).
6. **Readiness RPC (A8).** `get_node_version` returns the scheduled next version and height on a schedule with one future entry, and omits them otherwise.
7. **Spec and invariants.** `cargo xtask spec-coverage` (new rules in CON-14/15 and AA-1..AA-6 from the branches), `cargo xtask claims-check`, `cargo xtask coverage --verify-targets` in CI solely (AGENT-RULES).

NOT VERIFIED in this ADR: none of the above has been run for v2. This ADR is documentation alone; the frame counts are arithmetic on measured sizes from #447, and the cadence finding is from reading the defaults.

## Open questions for Eric

1. **Circulating supply (A1).** What counts as circulating: the mining reserve excluded, the treasury excluded, both? The code has no such value today.
2. **W1 stability window.** The brief names it as "genesis + 3 months". The repo uses "W1" for a different item (`.claude/plans/zero-trust-analysis-2026-11-03.md`: one machine is the whole network). Confirm the 3-month rule and its name.
3. **Block cadence.** Confirm the intended cadence (500 ms round, or 1 s as the docs say). The lead and the 3-month floor are in blocks, so the answer changes them by 2×.
4. **Unbonding window (A7).** The genesis value and epoch length in blocks, so ADR-096 can derive the exit window.
5. **Bit 8 on mainnet (A6).** Stays off until a separate audited release. Confirm that v2 ships with the flag `false` and the bit defined but unscheduled.
6. **Bit 9 rollout (A5).** A proposal naming bit 9 must not execute until every validator runs a binary that knows bit 9. Confirm the operator runbook gate for this.

## Consequences

- v2 is a set of draft PRs with one ordering and one activation rule, instead of four independent ones.
- The block-cadence finding must be resolved before any lead or floor value is treated as final.
- A6 keeps the shielded pool off under v2. Flipping `CIRCUIT_IS_AUDITED` is its own change.
- A8 gives operators a way to see an upcoming halt before it happens.

## Rejected alternatives

- **One combined PR for all eight items.** Rejected: the shielded flag and the account-authority wire format have different review gates (audit and two human reviewers), and merging them would drag the audit requirement onto A5.
- **Flip `CIRCUIT_IS_AUDITED` in v2.** Rejected: it is a consensus change with an audit requirement, and none has been run.
- **Keep the lead in blocks without a cadence measurement.** Rejected: the "30 days" claim is not supported by the code's default round interval.
