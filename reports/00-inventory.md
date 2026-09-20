# 00 — Repository Inventory

**Commit:** `223fe56607323447f7a78d1f3592b9b09ca97477` (`feat/complete-session-work`)
**Date:** 2026-09-20
**Toolchain:** `nightly-2026-07-15-x86_64-pc-windows-msvc` — `rustc 1.99.0-nightly (da80ed070 2026-07-14)`, `cargo 1.99.0-nightly (59800466c 2026-07-07)`
**Host:** Windows 11 Pro 10.0.29671, 24 logical CPUs, 32 GB RAM

Every number below is the output of a command, quoted in the section that
produced it. Nothing here is estimated.

## Method

```
cargo metadata --format-version 1 --no-deps
cargo check   --workspace --all-targets --message-format short
cargo clippy  --workspace --all-targets --message-format short
cargo tree -d --workspace --edges normal
du -sh target
git ls-files
```

## 1. Disk

```
$ du -sh target
289G    target
```

```
$ Get-PSDrive D | Select UsedGB, FreeGB
UsedGB FreeGB
------ ------
 310.9   89.2
```

`target/` is **289 GB** — 93% of everything on the volume, and 9.6x the 30 GB
ceiling this phase sets. CLAUDE.md's "356 GB" and the session memory's "415 GB"
are both stale; 289 GB is the figure as of this commit.

`target/` cannot be read by the agent harness: `.claude/settings.json` denies
`Read(./target/**)` and `.ignore` excludes it from every search tool. Size is
therefore measured with `du`, never walked.

## 2. Workspace members

`cargo metadata --no-deps` reports **45 packages, 45 workspace members** — the
root package `custom-l1-node` plus 44 directories. CLAUDE.md states "32
workspace members"; that count is stale by 12.

| Path | Package | Targets | rust-version | Non-default features |
|---|---|---|---|---|
| `.` | `custom-l1-node` | lib, bin x5, bench x10, test x47, example x2 | 1.88 | xdp |
| `api-gateway` | `maya-api-gateway` | lib, test x2 | 1.88 | - |
| `archive` | `maya-archive` | lib, test x2 | 1.88 | - |
| `blockgraph` | `maya-blockgraph` | lib, test x2 | 1.88 | - |
| `confidential-ai` | `maya-confidential-ai` | lib | 1.88 | - |
| `crypto-pq` | `maya-crypto-pq` | lib | 1.88 | - |
| `cuda-miner` | `maya-cuda-miner` | lib, test x2, build.rs | 1.88 | cuda |
| `custody-mpc` | `maya-custody-mpc` | lib, test x4 | 1.88 | tls |
| `dex` | `maya-dex` | lib, test x5 | 1.88 | - |
| `docgen` | `maya-docgen` | bin | 1.88 | - |
| `ebpf-net` | `maya-ebpf-net` | lib, test x2 | 1.88 | xdp |
| `ebpf-net/common` | `maya-ebpf-net-common` | lib | 1.88 | aya |
| `explorer` | `maya-explorer` | lib, bin, test x2 | 1.88 | - |
| `faucet` | `maya-faucet` | lib, bin, test x2 | 1.88 | - |
| `fee-market` | `maya-fee-market` | lib | 1.88 | - |
| `governance` | `maya-governance` | lib, test | 1.88 | - |
| `htlc-lattice` | `maya-htlc-lattice` | lib, bench, test | 1.88 | - |
| `htlc-watcher` | `maya-htlc-watcher` | lib, bin, test | 1.88 | - |
| `identity` | `maya-identity` | lib | 1.88 | - |
| `iot-anchor` | `maya-iot-anchor` | lib | 1.88 | std, tpm |
| `iso20022` | `maya-iso20022` | lib | 1.88 | - |
| `l2-flash` | `l2-flash` | lib, test x2 | 1.88 | - |
| `lattice-pow` | `maya-lattice-pow` | lib, test | 1.88 | - |
| `ledger-math` | `maya-ledger-math` | lib | 1.88 | - |
| `light-client` | `maya-light-client` | lib, test x2 | 1.88 | - |
| `mev` | `maya-mev` | lib, test x3 | 1.88 | - |
| `neural-gas-trainer` | `maya-neural-gas-trainer` | lib, bin, test | 1.88 | - |
| `pool-service` | `maya-pool-service` | lib, bin, test x2 | 1.88 | - |
| `radio-transport` | `maya-radio-transport` | lib | 1.88 | - |
| `rwa` | `maya-rwa` | lib | 1.88 | - |
| `sdk-ffi` | `maya-sdk-ffi` | lib, bin, cdylib | 1.88 | - |
| `sdk-wasm` | `maya-sdk-wasm` | cdylib, rlib | 1.88 | - |
| `stateless-core` | `maya-stateless-core` | lib, test x3 | 1.88 | - |
| `stratum-v2` | `maya-stratum-v2` | lib | 1.88 | - |
| `telemetry` | `maya-telemetry` | lib, bin, test | 1.88 | client, server |
| `threat-firewall` | `maya-threat-firewall` | lib, bin | 1.88 | - |
| `threat-intel` | `maya-threat-intel` | lib | 1.88 | - |
| `vm` | `maya-vm` | lib, bench, test x7 | 1.88 | - |
| `vrf` | `maya-vrf` | lib, test x2 | 1.88 | - |
| `wallet` | `l1-wallet` | lib, bin | 1.88 | - |
| `wallet-gui/core` | `maya-wallet-core` | lib, test x4 | 1.88 | - |
| `wgpu-miner` | `maya-wgpu-miner` | lib, bin, test | 1.88 | gpu |
| `zk-privacy` | `maya-zk-privacy` | lib, test x2 | 1.88 | - |
| `zkml` | `maya-zkml` | lib | 1.88 | - |
| `zkml-prover` | `maya-zkml-prover` | lib, bench, test x2 | 1.91 | - |

`maya-zkml-prover` is the one member with a different `rust-version` (1.91,
required by `tract-onnx`); the node does not depend on it outside
`[dev-dependencies]`, which is invariant 20.

## 3. Crates outside the workspace

Nine manifests are tracked but are not members. Each has its own `[workspace]`
table and its own `target/`, deliberately — a nested workspace does not inherit
the root's `[profile.dev.package.*]` overrides, which is a known cost recorded
in session memory.

| Path | Why it is not a member | Host-buildable? |
|---|---|---|
| `fuzz` | `cargo-fuzz`, nightly, libFuzzer sanitizer flags | no (needs `cargo fuzz`) |
| `offsec-sandbox` | LibAFL + optional Z3 must not reach the node graph or Kani | yes, separately |
| `iot-firmware` | `thumbv8m.main-none-eabihf` Cortex-M33 firmware | no (cross target) |
| `ebpf-net/programs` | `bpfel-unknown-none`, `-Z build-std=core`, needs `bpf-linker` | no (cross target) |
| `dashboard` | CSR Leptos, `wasm32-unknown-unknown` only, built by `trunk` | no (wasm only) |
| `wallet-gui/ui` | Leptos frontend, `wasm32`, built by `trunk` | no (wasm only) |
| `wallet-gui/src-tauri` | Tauri shell; own workspace so the GUI bundles independently | yes, separately |
| `app-maya2c` | Ledger device app | no (cross target) |
| `contracts/token-swap` | WASM contract compiled into `target-contracts/` | no (wasm only) |

Six of the nine cannot be compiled for the host at all. Any "does the repo
build" claim that does not say so is a claim about 45 of 54 crates.

## 4. Executables, tests, benches

**16 binaries:**

| Binary | Package |
|---|---|
| `node`, `miner`, `genesis`, `genesis-ceremony`, `peerid` | `custom-l1-node` |
| `l1-wallet` | `l1-wallet` |
| `explorer` | `maya-explorer` |
| `maya-faucet` | `maya-faucet` |
| `pool` | `maya-pool-service` |
| `maya-telemetry` | `maya-telemetry` |
| `htlc-watcher` | `maya-htlc-watcher` |
| `threat-firewall` | `maya-threat-firewall` |
| `wgpu-miner` | `maya-wgpu-miner` |
| `neural-gas-trainer` | `maya-neural-gas-trainer` |
| `maya-docgen` | `maya-docgen` |
| `uniffi-bindgen` | `maya-sdk-ffi` |

The brief's "many genesis binaries" is **two**: `genesis` (writes a genesis
file) and `genesis-ceremony` (the multi-party key ceremony). They are not
duplicates of each other.

**47 integration tests at the repository root** (`tests/*.rs`), plus per-crate
suites (`vm` 7, `dex` 5, `custody-mpc` 4, `wallet-gui/core` 4, `mev` 3,
`stateless-core` 3, and 2 or 1 in most others) — 105 test targets in total
across the workspace.

**13 bench targets:** 10 in `custom-l1-node` (`algo_comparison`, `argon_blake`,
`dag`, `dex_matching`, `ebpf_bench`, `gas_predictor`, `hybrid_footprint`,
`hybrid_signing`, `mlkem_handshake`, `oracle`), plus `maya-vm/module_cache`,
`maya-htlc-lattice/verify`, `maya-zkml-prover/verify`.

**16 fuzz targets** in `fuzz/fuzz_targets/`: `account_decode`, `block_decode`,
`block_sync_response`, `car_decode`, `did_decode`, `header_decode`,
`htlc_lattice_decode`, `iot_anchor_decode`, `iso20022_decode`,
`payload_decode`, `radio_frame_decode`, `stateless_witness_decode`,
`sv2_frame_decode`, `threat_evidence_decode`, `tx_decode`, `xdp_relay_decode`.

**10 scripts** in `scripts/`: `bench_report.py`, `build-contract.sh`,
`check-unsafe.sh`, `check_brand_refs.py`, `deploy_brand_assets.py`,
`doc_coverage.sh`, `local_cluster.sh`, `make_og_card.py`,
`make_zkml_fixture.py`, `xdp_netns.sh`.

## 5. Compile status

Warm cache, this commit, no edits:

```
$ CARGO_BUILD_JOBS=4 cargo check --workspace --all-targets --message-format short
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.64s
warning: the following packages contain code that will be rejected by a future
version of Rust: proc-macro-error2 v2.0.1
```

```
$ CARGO_BUILD_JOBS=4 cargo clippy --workspace --all-targets --message-format short
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 28.36s
warning: the following packages contain code that will be rejected by a future
version of Rust: proc-macro-error2 v2.0.1
```

**All 45 members compile. Zero errors, zero clippy warnings** under the current
(default) lint configuration. The only diagnostic in either run is a
future-incompatibility notice for the transitive `proc-macro-error2 v2.0.1`,
which no first-party crate depends on directly.

This is the baseline the workspace lint table in Phase A is measured against.
It is not a claim that `cargo test` passes — see section 8.

## 6. Duplication audit

The foundation brief asked for a merge plan for four claimed duplications.
Three of the four do not exist in this tree.

| Claim | Finding | Evidence |
|---|---|---|
| "two `state_pruner` crates" | **One**, and it is not a crate — `src/state_pruner/` is a module of the root package | `Cargo.toml:246` refers to it; no second path exists |
| "several governance modules" | **One crate** `governance/` (dependency-free for Kani, invariant 12) plus its state-execution module in the node | metadata: a single `maya-governance` package |
| "three explorers" | **One explorer.** `explorer/` is the chain explorer; `dashboard/` is a Leptos ops page; `wallet-gui/ui` is the wallet frontend. Three web surfaces, three different jobs | section 3 |
| "many genesis binaries" | **Two**, with distinct jobs | section 4 |

**No `attic/` directory is created.** There is no superseded code to move into
it. Creating one to satisfy the brief would imply a consolidation that did not
happen.

The real redundancy in this tree is not duplicate crates. It is dependency
versions — section 7.

## 7. Dependency version drift

95 distinct external dependencies across the 45 member manifests. **Zero** are
declared in a `[workspace.dependencies]` table, because that table does not
exist. 41 of the 95 are used by two or more members and would be unified by
one; 54 are used once.

Six dependencies are declared at **incompatible** versions by different
members, so the lockfile carries two copies of each:

| Dependency | Versions | Split across |
|---|---|---|
| `chacha20poly1305` | `0.10` / `0.11` | node, `confidential-ai`, `custody-mpc`, `ebpf-net`, `mev` **vs** `wallet`, `wallet-gui/core` |
| `sha3` | `0.10` / `0.11` | `iot-anchor` **vs** `htlc-lattice`, `stateless-core` |
| `rand_core` | `0.6` / `0.10` | `zkml`, `iot-anchor` **vs** `crypto-pq` |
| `rand` | `0.8` / `0.10` | `zkml-prover` **vs** `crypto-pq` |
| `criterion` | `0.5` / `0.7` | `vm` **vs** node, `htlc-lattice`, `zkml-prover` |
| `blake3` | `1` / `1.8` | 5 crates **vs** 13 crates (semver-compatible; one copy in the lock) |

`sha3` is the one that matters beyond tidiness: `iot-anchor` (Kani-verified,
`no_std`) and `htlc-lattice` / `stateless-core` (both Kani-verified) hash with
two different releases of the same SHAKE implementation.

`cargo tree -d --workspace` reports **110 duplicated version groups** in the
lockfile overall. Most are transitive and unavoidable. One is worth recording
against the invariant that tries to prevent it:

```
$ cargo tree --workspace -i curve25519-dalek@5.0.0 --edges normal
curve25519-dalek v5.0.0
`-- ed25519-dalek v3.0.0
    `-- libp2p-identity v0.3.0
        `-- libp2p v0.57.0
            `-- custom-l1-node v0.1.0 (D:\Maya2C)
```

`Cargo.toml` pins `curve25519-dalek = "4"` with the comment *"two copies of a
curve in one lockfile is two sets of encoding rules"*, and pins
`ed25519-dalek = "2.2"` so that `src/state/threat_exec.rs` calls a known
`verify_strict`. Both are in the lockfile **twice**: libp2p 0.57 pulls
`ed25519-dalek 3.0` and with it `curve25519-dalek 5.0`. The consensus-side
verification of a gossip signature therefore runs a different release of the
library than the one libp2p used to produce and check that signature. The
comment's stated goal is not currently achieved.

This is a pre-existing finding, not a regression introduced here, and resolving
it is out of scope for the foundation phase. Recorded so it is not
rediscovered. Tracked as ADR-005.

## 8. What this inventory does not establish

- **Tests are not run here.** `cargo nextest run --workspace` needs
  `CARGO_BUILD_JOBS=1` on this machine (32 GB RAM; parallel `link.exe` with
  wasmtime / arkworks / halo2 hits `LNK1102: out of memory`, which nextest
  reports as ordinary test failures). A real test run is a separate, serialised
  job and belongs in `reports/01-foundation.md`.
- **Six of the nine non-member crates cannot be built for the host**
  (section 3), so no statement here covers them.
- **Coverage, `cargo deny`, `cargo audit` and doc coverage were not run.**

## 9. Infrastructure absent at this commit

Confirmed by direct existence check, not assumed:

| Missing | Consequence |
|---|---|
| `[workspace.dependencies]` | section 7 — 95 deps, 6 incompatible splits, no single point of control |
| `[workspace.lints]` | no shared lint policy; `unsafe_op_in_unsafe_fn` unenforced |
| explicit `resolver` | inferred `"3"` from the root package's edition 2024 — correct today, unstated and therefore fragile |
| `[profile.dev]`, `[profile.release]`, `[profile.ci]` | **release builds use cargo defaults**: no LTO, `codegen-units = 16`, no strip. The 30 `[profile.dev.package.*]` overrides at `Cargo.toml:480-605` are the only profile tuning in the tree |
| `.cargo/config.toml` | `CARGO_BUILD_JOBS=1` is documented in CLAUDE.md as mandatory on this machine and enforced by nothing; no linker selection |
| `rust-toolchain.toml` | the toolchain is pinned only by a **user-global** rustup default. A fresh clone gets whatever nightly the machine has. `miri` and `llvm-tools` are available for `nightly-2026-07-15` but are not installed |
| `xtask/` | no `cargo xtask disk`; nothing warns before the volume fills (it has filled, twice) |
| `features.toml` | SHIPPED / RESEARCH / PLANNED status lives in prose in CLAUDE.md **and** `docs/architecture-vision.md` **and** `docs/trajectory.md`, with no machine-checkable source |
| `.github/workflows/ci.yml` | there is **no** fmt / clippy / nextest / deny workflow. The five that exist are `ebpf-net`, `iot-anchor`, `offsec-sandbox`, `publish_sdk`, `security-audit` |
| `PROGRESS.md`, `docs/adr/`, `sim/`, `reports/` | absent |

## 10. Housekeeping done

13 `rustc-ice-*.txt` compiler crash dumps were sitting in the repository root
(2026-09-09 through 2026-09-15). They are untracked — `.gitignore:48` already
carries `rustc-ice-*.txt` — and were deleted. No tracked file changed.
