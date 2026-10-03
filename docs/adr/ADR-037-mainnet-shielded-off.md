# ADR-037: Mainnet v1 launches with the shielded pool off

**Status:** Accepted (2026-10-03). Mainnet launch gate 2
(`docs/mainnet-v1-plan.md`), implementing Eric's 2026-09-30 decision
"mainnet v1 without private transfers".
**Date:** 2026-10-03

## Context

The shielded pool's join-split AIR has had no independent audit
(`docs/mainnet-readiness.md` §1). One missing constraint would let anyone
mint shielded value that no supply audit reveals. Four guards therefore
refused any value-bearing chain id outright: the node at startup, the genesis
ceremony, both Terraform module sets, and a test pinning
`CIRCUIT_IS_AUDITED = false`. That made mainnet wait on an audit nobody has
the money for yet, while investors wait on mainnet.

A pool that never runs cannot mint. The question is how to switch it off so
that no operator can quietly switch it back on.

## Decision

- **A genesis parameter, `shielded_activation_height`.** Absent, it means
  block zero (what every genesis written before this ADR meant) and leaves
  the genesis id unchanged, so maya-testnet-1 keeps running the pool and its
  id. Present, it is hashed into the genesis id
  (`GenesisConfig::chain_id_commitment`, domain
  `maya2c genesis shielded activation v1`), so two operators who disagree
  about the pool disagree about block zero, and — through ADR-036 — about
  every signature after it.
- **Mainnet sets it to `u64::MAX`** (`SHIELDED_NEVER`): never, until a later
  protocol upgrade schedules a height after an audit.
- **Every block runs under it.** `BlockContext` carries
  `shielded_activation`; the chain builds every context through
  `Chain::context_at` from `ChainConfig::shielded_activation`, which the node
  sets from the genesis. Before activation a join-split fails the whole block
  with `NodeError::ShieldedInactive`, checked before any proof work.
- **The guards relax for exactly that configuration.** The node refuses a
  value-bearing chain only if its pool can run on an unaudited circuit
  (`check_shielded_guard`). The ceremony mints a mainnet genesis with the
  pool off rather than refusing it. Terraform, which cannot read a genesis,
  needs the operator to state `mainnet_shielded_pool_off = true`; the node
  still checks.

## Alternatives rejected

- **A compiled-in activation constant**, like the other `*_ACTIVATION_HEIGHT`
  values. It cannot differ between the testnet (which should exercise the
  pool) and mainnet without a cargo feature, and a feature that changes
  consensus makes two honest builds compute different state roots
  (ADR-002).
- **Removing the pool from mainnet builds.** Same objection, and it would
  make turning it on later a new binary rather than a scheduled height.
- **Relaxing the guards by chain id alone.** A guard that trusts a name
  would let a mainnet genesis with the pool on through.

## Consequences

- Mainnet can launch before the audit; private transfers come later, at a
  scheduled height, through a protocol upgrade and its own ADR.
- A shielded transaction sent to mainnet sits in the mempool until the
  builder drops it; the mempool does not stage payloads. It costs the sender
  nothing on chain and the node one failed staging.
- The genesis ceremony still builds only proof-of-work geneses
  (`bft: None`). A DAG-BFT mainnet genesis needs it to assemble a committee
  first — gate 5's work, not this ADR's.

## Evidence

- `shielded_tests::a_pool_that_is_off_refuses_a_joinsplit_and_changes_nothing`
- `genesis_tests::the_shielded_activation_is_bound_into_the_genesis_only_when_named`
- `genesis_tests::a_chain_runs_every_block_under_its_genesis_shielded_activation`
- `maya2c-node` `tests::mainnet_with_the_pool_off_starts_on_an_unaudited_circuit`
  and its two siblings; `genesis-ceremony` `shielded_tests`.
- End to end, 2026-10-03: `genesis-ceremony --chain-id maya-mainnet` wrote
  `shielded_activation_height: 18446744073709551615` and "shielded pool off"
  on its commitment sheet; `maya2c-node` started on that genesis; the same
  genesis with the field removed was refused with
  `UnauditedShieldedCircuit { network: "maya-mainnet" }`.
