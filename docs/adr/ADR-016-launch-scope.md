# ADR-016: Launch scope freeze — what mainnet v1 is, and what waits

**Status:** Accepted
**Date:** 2026-09-27
**Revisited by:** Master Prompt 21 (§ "Adoption-critical additions" below)

> **Update 2026-09-28:** Launch blocker 1 is done, so a `production` build now starts, in `dag-bft` mode only (`bins/maya2c-node/src/main.rs`). See [ADR-027](ADR-027-dag-bft-in-the-node.md). The text below is the decision as recorded; it is not current status.

## Context

Master Prompt 11 asks for the mainnet v1 core to be frozen, everything else
deferred with an activation path, a `production` build that cannot carry a
SIM or RESEARCH switch, and dag-bft as the only production consensus mode.
The reality audit (`reports/11-reality-audit.md`) is the evidence.

The largest fact the audit surfaced: **the node's only consensus mode is
ArgonBlake proof of work.** DAG-BFT exists as an engine (`crates/dag-bft`,
ADR-015) run under the simulator, not in `custom-l1-node`. A production
build therefore cannot run at all today — `maya2c-node --features
production` refuses to start with a message naming this ADR. That is the
correct behaviour, not a bug: the alternative is a mainnet binary running a
devnet mode.

## Decision

### Mainnet v1 core

| Component | Crate(s) | Why it is core | Status today |
|---|---|---|---|
| PQ signatures ML-DSA-65/87, SLH-DSA, hybrid | `crypto-pq`, `crates/node/src/crypto` | every transaction | verified (NIST ACVP) |
| ML-KEM + hybrid handshake | `crypto-pq`, `network::pq` | peer transport | verified; dual-KEM off by default |
| Types, encoding, state, storage | `crates/node` (`core`, `state`) | the state machine | shipped |
| Fee market | `fee-market` | spam defence, burn | **RESEARCH** (activation `u64::MAX`) — must activate before launch |
| Consensus: dag-bft only | `dag-bft` → node | ordering and finality | **engine only, not wired** — blocks launch |
| Mempool | `crates/node` | ingress | shipped (PoW-shaped) |
| Staking and slashing | — | validator set for dag-bft | **not built** — blocks launch |
| WASM VM, single Cranelift tier + cache | `vm` | contracts | shipped; Pulley tier exists, not used |
| Governance | `governance`, `crates/node/src/governance` | parameter changes | shipped |
| p2p over TCP/QUIC | `crates/node/src/network` | gossip | shipped |
| RPC | `crates/node/src/rpc`, `api-gateway` | clients | shipped |
| Native transfers, m-of-n multisig custody | `crates/node`, `crypto-pq::multisig` | value transfer | shipped (live from genesis, ADR-013) |

Three rows block a launch and are the first engineering work after this
freeze: wire dag-bft into the node, build staking/slashing, activate the fee
market. Each needs a spec section and conformance vectors (Master Prompt 15)
before it changes consensus.

### Deferred — stays in the tree, keeps its tests, activates later

| Module | Why deferred | Evidence that brings it in | Audit it needs |
|---|---|---|---|
| Multi-VM (EVM, SBF, Move) | not built; gas semantics unresolved | revm hosted with a gas-parity suite | VM + EVM equivalence |
| zkML | retired with halo2/KZG (ADR-008) | a transparent, PQ zkML verifier under 10 ms | cryptography |
| Neural gas | trained on synthetic data only | a model trained on testnet traffic that beats plain EIP-1559 in `econ` | economics + determinism |
| Agents, confidential AI | no on-chain caller | a use with measured demand | TEE + economic |
| DeFi / RWA / CBDC / identity modules | dark at `u64::MAX` | per-module spec + invariant hooks + load tests | state machine, per module |
| DePIN, IoT anchors | hardware SIM | real device fleet on testnet | hardware + firmware |
| Exotic transports (FSO, acoustic, neutrino, QKD) | SIM models only (`link-sim`) | a real modem/link | network |
| PoUW, lattice PoW | no measured useful output (ADR-015) | a published use of the work | consensus + economics |
| argonblake-pow | devnet mode | — (never mainnet) | — |
| Frontier HAL | SIM | — | — |
| Shielded pool | circuit unaudited (`CIRCUIT_IS_AUDITED = false`) | external cryptography audit | cryptography (Master Prompt 20 scope a) |
| Threshold lattice custody | Raccoon port, dealerless keygen is our own construction (ADR-014) | peer review of the keygen | cryptography |
| HQC KEM | draft standard; decapsulation leaks (ADR-009) | a constant-time implementation | cryptography |

Each activates through governance at a written-down height, never by binary
version (Master Prompt 15 §4).

### The production build

- `cargo build -p maya2c-node --features production` is the only mainnet
  build. The feature forwards to `crates/build-guard`, whose `compile_error!`
  fires if any `sim`, `research`, `frontier`, `test-util` or `insecure`
  switch is unified into the same build (`maya-entropy/sim-sources`,
  `maya-crypto-pq/hqc`, `maya-custody-mpc/threshold-lattice`,
  `maya-tiers/frontier` forward to it today).
- `cargo xtask release-check` builds it, scans `cargo tree -e features` and
  the symbol table, and has a `--force-sim` mode that must fail.
- In a production build, `--mine` is refused and the binary refuses to start
  in any consensus mode but dag-bft.
- **Brief vs tree (reported, not worked around):** the brief says the check
  must prove "no SIM/RESEARCH crate is linked". RESEARCH crates that write
  records under the state root are linked by the node *on purpose*, dark at
  `u64::MAX` (ADR-002: gating them by cargo feature would make two honest
  nodes compute different state roots). The check forbids SIM crates and
  every research/sim *feature*, and lists the RESEARCH crates linked dark so
  the linkage is visible.

### Adoption-critical additions (Master Prompt 21 §5)

Master Prompt 21 asks whether the adoption-critical parts of Prompts 22–24
belong in launch core. Decision: **smart-account validation with policy
modules (22 §1) and the contract resource/capability checks (23 §1) are
considered core**, because retrofitting account and asset semantics after
launch is exactly the legacy problem those prompts exist to avoid; both are
libraries today (`smart-account`, `contract-safety`) and join the three
blocking rows above. The developer platform (24) is tooling, not consensus,
and ships on its own schedule.

## Consequences

- The mainnet binary cannot start today, by design, and says why.
- The launch-blocking list is short and concrete: dag-bft in the node,
  staking/slashing, fee-market activation, smart-account validation, resource
  checks in the VM.
- READINESS.md tracks every core row with evidence or an explicit gap.

## Revisit when

Any launch-blocking row lands, or an external audit changes a deferral.
