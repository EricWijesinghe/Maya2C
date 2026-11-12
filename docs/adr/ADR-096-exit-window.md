# ADR-096: Exit window derivation for scheduled upgrades

Status: Accepted (post-genesis, v2). Claim: `096` in `D:/Temp/claude-locks/adr-claims.txt`.
Related: ADR-082 (genesis-committed upgrade levers), ADR-030 (vault accounts), ADR-028 (staking), invariant 34.

## Context

Invariant 34 says a scheduled protocol upgrade activates no sooner than
`MIN_UPGRADE_LEAD_BLOCKS` after its last possible execution. The lead exists so
that a holder who disagrees with a passed upgrade can leave before it binds.
The lead was chosen as 30 days (`crates/governance/src/limits.rs`) without a
written statement of what it must cover. This ADR writes that statement and
checks it.

## Derivation

Let `E` be the height a passed `ScheduleUpgrade` executes at and `A` its
activation. The lead is `A - E >= MIN_UPGRADE_LEAD_BLOCKS`. A holder who wants
out at execution needs every exit path to complete by `A`:

| Exit path | Blocks after `E` | Source |
|---|---|---|
| Vault withdrawal (delay chosen by the holder, at most the ceiling) | `vault::MAX_DELAY_BLOCKS` = 1,000,000 | `crates/node/src/state/vault.rs:31` |
| Governance stake unlock | 0 | A voter's stake is required to stay locked only until `executable_from`, which is at or before `E` (`crates/governance/src/proposal.rs:192`, `crates/node/src/state/governance_exec.rs:508`). Nothing is held after execution. |
| Staking unbond | `unbonding_epochs` x `epoch_blocks` = 7 x `epoch_blocks` | `crates/staking/src/params.rs:43`; `unbonding_epochs` is not overridable by genesis (`crates/node/src/genesis.rs:373`); `epoch_blocks` is a genesis value |

The sum is the requirement, which is conservative: a holder can take one path
per unit of stake, so the largest term alone would suffice. The sum is kept
because it costs nothing and does not need a claim about which paths compose.

Required inequality:

    MIN_UPGRADE_LEAD_BLOCKS >= MAX_DELAY_BLOCKS + 0 + 7 * epoch_blocks

With the compiled values, `2_592_000 >= 1_000_000 + 7 * epoch_blocks`, so the
inequality holds exactly when `epoch_blocks <= 227_428`.

## Check against the genesis values in the tree

| Network | `epoch_blocks` | Left side | Holds |
|---|---|---|---|
| Testnet (`docs/site/public/testnet/genesis.json:33`) | 3,600 | 1,025,200 | yes |
| Devnet (`scripts/join-devnet-genesis.sh`, `infra/testnet-vm/install.sh`) | 20 | 1,000,140 | yes |

## Enforcement

The inequality cannot be a compile-time assertion in `maya-governance`:
governance does not depend on the node, the vault or staking, and the
`epoch_blocks` term is genesis data, not a constant. It is pinned by a test in
`crates/node/tests/exit_window_tests.rs`, which uses the real constants and
asserts both the testnet value and the boundary (227,428 passes, 227,429 fails).

## Finding (not changed)

`epoch_blocks` has no upper bound in genesis validation
(`crates/node/src/genesis.rs` checks only that it is positive). A genesis with
`epoch_blocks` above 227,428 satisfies every local check and silently makes the
exit window shorter than the vault delay plus unbonding. The fix is a genesis
rule, which changes what genesis is accepted and so is a consensus decision.
That decision is left to the owner. Until it is made, the mainnet genesis must
keep `epoch_blocks <= 227,428`.

## Consequences

- No constant changes. `MIN_UPGRADE_LEAD_BLOCKS` stays at 2,592,000.
- A future change to the vault ceiling, the unbonding epochs, or the default
  epoch length has to re-run the pinned test, which fails if the inequality breaks.
