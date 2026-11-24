# Architecture

How the Maya2C node is put together, read from the code. Status — what works,
what is SIM or RESEARCH — is not restated here: `features.toml` and
[architecture-vision.md](architecture-vision.md) are the authorities for that,
and [REPO-MAP.md](REPO-MAP.md) maps every directory. This page follows one
transaction and one block through the node, and names the file that does each
step.

## The shape in one paragraph

Maya2C is a single-binary L1 node (`bins/maya2c-node`) built on one large
library crate (`crates/node`, package `custom-l1-node`). Ordering and finality
come from **DAG-BFT** (Narwhal-style certified DAG with the Bullshark commit
rule, `crates/dag-bft`; ADR-015, ADR-027) run by a staked validator committee
with **ML-DSA-87** validator keys (ADR-051). Every node derives each block itself from
committed certificates; blocks are never trusted from a peer. User
transactions are signed with a **hybrid ML-DSA-65 + SLH-DSA-SHA2-128s**
signature bound to the chain's genesis (ADR-036). Proof of work does not order
anything: miners earn through a separate **DAG-PoW mining rewards lane**
(ADR-044), with verifiable useful compute designed in ADR-079 / ADR-129.

## Crates the node is built from

| Crate | Role |
|---|---|
| `crates/node` (`custom-l1-node`) | everything consensus-shaped: blocks, transactions, state, the BFT driver, P2P, RPC |
| `crates/dag-bft` (`maya-dag-bft`) | the sans-IO DAG-BFT engine: `Validator`, `Committer`, `Certificate`, `Vertex` |
| `crates/crypto-pq` | post-quantum signature suites and the suite registry (invariants 2, 29, 30) |
| `crates/signer` | remote signer service with its own slashing protection (ADR-033), and the sealed key format `maya_signer::sealed` (ADR-049) |
| `crates/ledger-math`, `crates/amount` | checked 256-bit arithmetic (ADR-046: amounts are U256) |
| `crates/fee-market`, `crates/staking`, `crates/governance`, `crates/dex`, `crates/vm`, `crates/vrf` | state-machine subsystems linked into execution |
| `bins/maya2c-node` | the daemon: wiring, the BFT loop, block import, pruning, key files |

`ledger-math` and `dex` are kept free of C/C++ and heavy dependencies so Kani
can check them (invariants 1 and 6); `governance` carries invariant 12.

## Process layout (`bins/maya2c-node`)

| File | What it owns |
|---|---|
| `main.rs` | CLI flags, genesis load, state open, P2P start, RPC server, task wiring |
| `bft.rs` | the DAG-BFT loop: gossip frames and a clock in, blocks out (ADR-027) |
| `import.rs` | getting blocks into the chain from gossip and from a peer's history (orphan buffer) |
| `rejoin.rs`, `safety_guard.rs` | which trust a catch-up may use; refuses to start a validator that lost its safety log |
| `pruning.rs` | open or bootstrap the chain, archive/snapshot services |
| `validator_key.rs` | the validator key file: sealed at rest, or legacy plaintext (refused by a production build) |

## Life of a transaction

1. **Wire.** `crates/node/src/core/transaction.rs` — `Transaction` with
   inputs, outputs, nonce, a `TxKind` payload, and one authorisation: hybrid
   signature (v5/v6), suite-tagged (`SuiteAuth`, v7) or m-of-n multisig
   (`MultisigAuth`, v8). The signed bytes include a `ChainTag` derived from
   the genesis id, so a signature is valid on one chain only (ADR-036).
   Decoders (`from_bytes`) are hand-written and bounds-checked.
2. **Admission.** RPC `send_raw_transaction` or gossip on the transaction topic
   reaches `network/mempool.rs`, which checks signature, sender funds and
   nonce against current state before pooling. Gossip validation runs in
   `network/node/guard.rs`; peers that send invalid data are scored and
   quarantined by `network/peer_health.rs`.
3. **Batching.** The daemon's BFT loop (`bins/maya2c-node/src/bft.rs`) hands
   each validator its share of pooled transactions, and the driver
   (`BftDriver::submit`, `consensus/bft/driver.rs`) puts them in the batch of
   its next DAG vertex.
4. **Ordering.** The engine (`crates/dag-bft/src/validator.rs`) gathers a
   quorum of `n − f` votes (`W − f` stake on a weighted committee,
   `Committee::quorum`, ADR-039) per vertex into a certificate, and the committer
   (`commit.rs`) commits an anchor plus its causal history as a `SubDag`.
5. **Block.** `consensus/bft/builder.rs` turns one committed sub-DAG into
   exactly one block. Every node runs it on the same sub-DAG and parent
   state, so every honest node builds the same block id.
6. **Execution.** `Chain::insert_block` (`consensus/chain.rs`) runs the
   admission checks (tx root, size cap — invariant 33, parent, halt schedule
   — invariant 34, prune horizon) and applies the block through
   `state/` with an undo journal (`state/undo.rs`). The block's declared state
   root must equal the root execution produces, or the block is refused whole
   (invariant 24).

## State

`crates/node/src/state/` — `StateDB` on RocksDB (`db.rs`), a binary Merkle
tree over the account set (`merkle.rs`), the block store (`blocks.rs`) in the
same database. Each subsystem keeps its records under its own key prefix,
every prefix committed by the state root (invariant 25): accounts, staking,
fees and mining under `k:` (`k:cmt:`, `k:fee`, `k:mine`, `k:mseen:`), DEX,
identity, RWA, IoT anchors, threat intelligence, vaults (`q:`, invariant 32),
council and upgrade schedule (invariant 34). Most subsystems have an
`*_exec.rs` with their transition rules (`staking_exec.rs`, `dex_exec.rs`,
`mining_exec.rs`, …). `preview.rs` keeps the overlay and root a builder staged
for its own block, so applying that block does not stage it again; the root is
still compared on every apply. Consensus code uses no
floats, no wall-clock and no `HashMap` iteration order (Standing Order 4).

## Finality and catch-up

- **Attestations** (`consensus/bft/attest.rs`, ADR-038): each validator signs
  every block it built; `n − f` signatures (or `W − f` stake on a
  stake-weighted chain, ADR-040) make a **checkpoint**. The `Collector`
  flags a validator that attests two blocks at one height; a validator that
  signs two proposals for one slot is kept as equivocation evidence on disk
  (`evidence.rs`).
- **Catch-up**: a node more than the engine's 50-round window behind imports
  blocks up to a checkpoint and re-executes each (`catchup.rs`), or replays
  the epoch's certificates from a peer (`certsync.rs`, ADR-052). The serving
  peer is trusted for nothing: the quorum names the chain, re-execution checks
  the state.
- **Safety across restarts**: `store.rs` fsyncs each own proposal and vote, so
  a restarted validator cannot sign twice for one slot.
- **Remote signing**: `remote.rs` sends the slot and vertex digest to
  `maya2c-signer` in another process, so a node configured for it never holds
  the validator key (ADR-033).

## Networking

`crates/node/src/network/` — libp2p with gossipsub (blocks, transactions,
and BFT frames with attestations on the BFT topic; topics in `topics.rs`),
Kademlia discovery, Noise plus a second ML-KEM-keyed encryption layer
(`pq/`, see [CRYPTO_WATCH.md](CRYPTO_WATCH.md) for what it covers),
request/response block sync (`sync.rs`), and peer scoring and quarantine (`gossip_score.rs`,
`peer_health.rs`). Every connection's Noise prologue binds the chain tag
(`network_prologue`, `network/node/build.rs`), so nodes of different chains
refuse each other at the handshake. BFT frames are
encoded by `consensus/bft/wire.rs`: one canonical encoding per message,
length prefixes bounded against the bytes actually present.

## Mining lane (useful work)

DAG-BFT orders blocks; miners never do. A miner earns by putting a
`MiningShare` (`core/mining_payload.rs`) on chain whose DAG proof of work
meets the epoch's share target (`state/mining.rs`, `state/mining_exec.rs`,
ADR-044). Rewards are transfers out of a genesis `MINING_RESERVE` address no
key derives, so nothing is minted. Verifiable useful compute (AI inference,
proofs) is designed in ADR-079 and ADR-129 and is not on the consensus path;
see `features.toml` for its status.

## Fees, staking, governance

- Fees (`state/fees.rs`, `fee_exec.rs`, ADR-029): a base fee per serialized
  byte that burns, plus tips to validators.
- Staking (`state/staking.rs`, ADR-028): bonds, committee per epoch.
- Upgrades (`crates/node/src/upgrade.rs`, ADR-082): protocol versions, feature
  bits, and halt-before-fork; a subsystem is switched off by an activation
  height of `u64::MAX`, never by a cargo feature (ADR-002).

## RPC and gateways

`crates/node/src/rpc/` serves JSON-RPC (method table in `rpc/mod.rs`) with a
per-peer rate limiter (`rpc/limit.rs`). `crates/api-gateway` sits in front for public REST,
GraphQL and event streams with a default-deny allowlist and its own limits.
Apps (`apps/explorer`, `apps/wallet-gui`, `apps/hub`, …) talk to nodes through
these.

## Where to read further

| Topic | Document |
|---|---|
| invariants that must never break | [invariants.md](invariants.md) |
| normative wire and state spec | `spec/` |
| every design decision | `docs/adr/` |
| building, testing, disk and memory limits | [build.md](build.md), [DEVELOPER_GUIDE.md](DEVELOPER_GUIDE.md) |
