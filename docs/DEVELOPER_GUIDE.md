# Developer guide

From a fresh clone to a change that is ready for review. The measured build
figures and machine limits live in [build.md](build.md); the rules every change
follows live in [CLAUDE.md](../CLAUDE.md) and [invariants.md](invariants.md).
How the node fits together: [ARCHITECTURE.md](ARCHITECTURE.md).

## 1. Toolchain

- Rust is pinned by `rust-toolchain.toml` (`nightly-2026-07-15`, edition 2024,
  `rust-version = 1.88`). `rustup` installs it on the first `cargo` call.
- `.cargo/config.toml` sets `jobs = 4`. Keep it: wider builds exhaust memory
  and fail as `os error 1455` or `can't find crate` ([build.md](build.md)).
- Optional: `cargo-nextest`, `cargo-deny`, `cargo-fuzz` (nightly), `cargo-vet`.
  Kani runs under Linux/WSL only.

## 2. Build

Build one package at a time. The node and its tools:

```bash
cargo build -p maya2c-node        # the daemon (bins/maya2c-node)
cargo build -p l1-wallet          # CLI wallet: keystore, send
cargo build -p maya2c-peerid      # prints/creates a node's libp2p peer id
```

Do not interleave a `-p` build with a `--workspace` build or `cargo xtask`
in the same target directory: the different feature sets wedge it
(`required to be available in rlib format`). Run `cargo xtask disk` before a
long build — a full disk reports as `os error 112`, not "out of space".

A mainnet binary is built with `--no-default-features --features production`
(see `docs/build.md`, "Production build"); `cargo xtask release-check`
verifies that build.

## 3. Run a node locally

The quickest end-to-end check is the local DAG-BFT devnet. It starts four
validators and one observer as real `maya2c-node` processes on localhost,
sends transfers over JSON-RPC, and prints what it measured as JSON: each
node's height over time (blocks per second), the time from `send` to the
transfer being visible on every node, and whether every node agrees on the
block id at the common height. It exits non-zero when a check fails:

```bash
cargo xtask bft-devnet                 # dev profile
cargo xtask bft-devnet --release --seconds 60 --transfers 20
```

Five processes on one machine measure that the binary runs end to end, not
the chain's ceiling. Related runners: `cargo xtask localnet` (four validators
over libp2p, one killed and caught up; `--remote-signer` puts the keys in
`maya2c-signer`), `cargo xtask up` / `down` (validators, API gateway, chat
relay and a funded devnet wallet, left running), and
`scripts/local_cluster.sh` (N nodes from a genesis minted by the ceremony
tool). `cargo xtask help` lists the rest.

Node flags these runners use (see `maya2c-node --help`): `--genesis`,
`--data-dir`, `--rpc-addr`, `--p2p-port`, `--bootnode
/ip4/…/tcp/…/p2p/<peer id>`, `--validator-key`, `--allow-private-peers`
(loopback testing only), `--generate-validator-key <path>`.

## 4. Test

```bash
cargo test -p custom-l1-node --test bft_node_tests   # one integration test binary (crates/node/tests/<name>.rs)
cargo test -p custom-l1-node --lib           # the node's unit tests
cargo test -p maya-dag-bft                   # the BFT engine
```

Where tests live:

| Path | What |
|---|---|
| `crates/node/tests/` | integration tests per subject: `consensus_tests`, `bft_node_tests` (4 validators in memory), `network_tests`, `malleability_tests`, `exploit_replays`, … |
| `crates/node/tests/attack_simulation_tests.rs`, `byzantine_guard_tests.rs` | conflicting spends, low-work branches, difficulty clamps, sybil floods, peer scoring and quarantine |
| `crates/dag-bft/tests/` | adversarial schedules and property tests (proptest), forged votes and certificates, restarts from the safety log |
| `spec/` + `cargo xtask spec-coverage` | conformance vectors for consensus rules |
| `fuzz/` | cargo-fuzz targets (separate workspace) |
| `formal/` | Kani, Lean, Z3 proofs |

Chain tests use `ChainConfig::without_pow_verification()` so they do not
spend an Argon2id pass per block; `tests/common/mod.rs` provides
`open_chain` and `test_chain()` (the chain tag transactions sign for).

## 5. Gates before review

```bash
cargo xtask coverage --verify-targets   # features.toml claims name real tests
cargo deny check                        # advisories, bans, licences, sources
bash scripts/lint_debt.sh --check       # clippy::pedantic ratchet must not rise
cargo xtask proof-ratchet --check       # proofs, vectors, fuzz targets never fall
bash scripts/doc_coverage.sh --check    # doc coverage floor
cargo xtask claims-check                # no unbacked "first/only" wording
```

## 6. Rules a change must follow

- **Consensus code**: no floats, no wall-clock time, no `HashMap` iteration
  order, no `unsafe` without a `// SAFETY:` comment and a test. A
  consensus-behaviour change needs a `spec/` update, conformance vectors and an
  ADR.
- **Subsystems switch off by activation height** (`u64::MAX`), never by a
  cargo feature: every node must compute the same state root (ADR-002).
- **Secrets** are typed, zeroized and never logged; key files are created
  owner-only.
- **Numbers** in docs or commits are measured, with the command that produced
  them, or written MISSING.
- **New subsystem**: a status tag in `docs/architecture-vision.md` and a
  `features.toml` entry in the same change.
- **New dependency** used by another member goes in `[workspace.dependencies]`
  (ADR-005); a new core dependency needs a `cargo vet` entry.
- **Design decisions** get an ADR in `docs/adr/` when they are made.
- Commits: `<type>: <why>` (`feat`, `fix`, `refactor`, `docs`, `test`,
  `chore`, `perf`, `ci`).

## 7. Where to look first

| You want to change | Start at |
|---|---|
| transaction format or signing | `crates/node/src/core/transaction.rs`, invariant 31 |
| block building under BFT | `crates/node/src/consensus/bft/builder.rs` |
| block import / reorg | `crates/node/src/consensus/chain.rs`, invariant 24 |
| a state subsystem | `crates/node/src/state/<name>.rs` + `<name>_exec.rs` |
| P2P | `crates/node/src/network/` |
| RPC methods | `crates/node/src/rpc/server.rs` (method table in `rpc/mod.rs`) |
| the daemon's wiring | `bins/maya2c-node/src/main.rs`, `bft.rs` |
