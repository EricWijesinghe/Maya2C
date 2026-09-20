# ADR-002: Feature tiers gate services, never consensus

**Status:** Accepted
**Date:** 2026-09-20

## Context

The foundation brief asked for three cargo feature tiers:

> `core` (default: crypto, state, consensus, vm, rpc), `extended` (DeFi, RWA,
> identity, bridges, AI), `frontier` (all hardware / space / bio / quantum HAL
> simulators). A default build compiles core only.

Read literally, `extended` would put `dex`, `rwa`, `identity` and the bridge
crates behind a cargo feature that is off by default.

Those crates are not optional in the sense the tier implies. Each of them
writes **committed records**: prefixes registered in
`state::commitments::RECORD_LAYERS`, under the state root, journalled for
reorg safety. That is critical invariant 25 — every persisted consensus record
is under the state root, and anything outside it is on an explicit local-only
list.

A node compiled without `extended` would therefore execute a different set of
record layers and compute a **different state root** from a node compiled with
it. Two honest operators running the same tagged release with different
feature flags would fork. The flag would be a consensus rule that nothing in
the protocol describes and no block commits to.

This tree already has the mechanism for turning a subsystem off, and it is not
a cargo feature. `ZKML_ACTIVATION_HEIGHT`, `STATELESS_ACTIVATION_HEIGHT`,
`HTLC_L_ACTIVATION_HEIGHT`, `THREAT_INTEL_ACTIVATION_HEIGHT` and
`IOT_ACTIVATION_HEIGHT` are all `u64::MAX`, and `FeeConfig::DISABLED` is the
same idea. Every node compiles the same code; the *chain* decides whether it
runs. Switching one on is a height, written down, that every node agrees about.

## Decision

A tier gates **binaries, services, client surfaces and simulators**. It never
gates a crate the node links for consensus.

Concretely:

- `core` — what a validating node needs: cryptography, state, consensus, the
  VM, RPC, peer-to-peer. This includes every crate that writes a committed
  record, whether or not that record type is currently reachable.
- `extended` — things that are separate processes or separate artifacts: the
  explorer, the faucet, the telemetry collector, the API gateway, the pool
  service, the wallets, the SDKs, the off-chain provers and trainers.
- `frontier` — hardware, space, bio and quantum work and their simulators:
  the GPU miners, `ebpf-net`'s Linux half, `radio-transport`, `iot-anchor`'s
  device side, and everything in the PLANNED list.

A subsystem is turned **off** by an activation height, not by a feature flag.
A subsystem is left **unbuilt** by a tier only when nothing it does can change
a state root.

The tier of each subsystem is recorded in `features.toml` and printed by
`cargo xtask coverage`.

## Alternatives considered

**Take the brief literally and gate the record-writing crates.** Rejected: it
creates a consensus split reachable from a build flag. The failure would not
be a compile error or a test failure; it would be two nodes disagreeing about
a state root in production, which is the most expensive class of bug this
codebase can have.

**Gate them, and add a genesis field recording which tiers were enabled.**
This would make the split explicit rather than silent, and it is what a chain
with genuinely optional execution layers would do. Rejected as premature: it
turns a build-system question into a protocol question, and nothing yet needs
a node that cannot execute a `TxKind` the chain defines.

**No tiers at all.** Rejected: `frontier` is genuinely useful. A CI runner
with no GPU, no CUDA toolkit and no XDP-capable NIC should not compile the
code that needs them, and the tree already does this per-crate (`cuda-miner`'s
`cuda`, `wgpu-miner`'s `gpu`, `ebpf-net`'s `xdp`, all default-off). The tiers
name that existing practice instead of inventing something.

## Consequences

- A default build compiles more than the brief's `core` would have. The
  saving comes from `frontier` and from the service binaries, not from the
  consensus crates.
- `features.toml`'s `tier` column is a statement about *build cost*, not about
  what a chain executes. The `class` and `status` columns are the ones that
  say whether something runs.
- If a future subsystem genuinely needs to be optional at the protocol level,
  it needs a genesis field and a state-root story, and that is a new ADR.

## Revisit when

Someone proposes a crate that writes a committed record *and* must be
optional at build time — or when a genesis-level "enabled layers" field is
designed, at which point the second alternative above becomes live.
