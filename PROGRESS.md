# Progress

What is done, what is next, and where to resume. Ticked only when the command
that proves it has been run and its output recorded — see Standing Order 1 in
`CLAUDE.md`.

Two plans run in parallel and they are not the same thing:

- **Master Prompts 1–10** below: the build phases. Prompt 1 is the workspace
  foundation; 2–10 are subsystem work.
- **`docs/trajectory.md`**: a 160-prompt ordering in twelve domains, completed
  through ~48. That file is the *plan*. `features.toml` and
  `docs/architecture-vision.md` are what the tree actually contains.

---

## Master Prompt 1 — Workspace foundation

Status: **Phase A complete, Phase B and C outstanding.**
Report: `reports/00-inventory.md`, `reports/01-foundation.md`.

### Phase 0 — audit

- [x] Inventory every crate, binary, test, bench and script — `reports/00-inventory.md`
- [x] Measure `target/` — 316 GiB at `223fe56`, 336.5 GiB across all artifact directories
- [x] Record compile status from real output — 45/45 members compile
- [x] Duplication audit — three of the four claimed duplications do not exist; no `attic/` created
- [x] Delete 13 untracked `rustc-ice-*.txt` crash dumps

### Phase A — governance layer, at current paths

- [x] `rust-toolchain.toml` — `nightly-2026-07-15`, components `rustfmt clippy rust-src llvm-tools miri`, three cross targets
- [x] `.cargo/config.toml` — `cargo xtask` alias; clang + mold on Linux, verified under WSL; Windows keeps `link.exe` with the reason written down
- [x] `[workspace.package]` — version, edition, rust-version, publish
- [x] `[workspace.dependencies]` — 40 entries, 450 declarations migrated, three version splits closed, three left split with reasons — ADR-005
- [x] `[workspace.lints]` — `unsafe_op_in_unsafe_fn = deny`, `clippy::pedantic = warn`, `clippy::unwrap_used = warn` (denied in CI on lib/bins), wired into all 45 members
- [x] Profiles — `dev`, `dev.package."*"`, `release`, `dist`, `ci`, `fuzz` — ADR-003
- [x] `cargo clean` and cold rebuild, measured — `reports/01-foundation.md`
- [x] Full test suite green: 2,468 tests across 167 binaries, 0 failed, 6 skipped
- [x] `cargo fmt --all` (157 diffs / 55 files) and `rustfmt.toml`
- [x] `.gitattributes` — the generated-weights drift check could not pass before it
- [x] `resolver = "3"` stated; `default-members` is the core tier
- [x] `xtask` — `cargo xtask disk` (30 GiB ceiling), `cargo xtask coverage`
- [x] `features.toml` — 90 entries; 43 verified, 30 working, 17 planned — ADR-004
- [x] `docs/adr/` — ADR-001 … ADR-005
- [x] `PROGRESS.md`
- [x] `CLAUDE.md` revision with the Standing Orders
- [x] `.github/workflows/ci.yml` and `nightly.yml`
- [ ] CI observed green on a real push (needs a push; nothing local can prove it)
- [ ] Bring the *sum* of artifact directories under 30 GiB. The root `target/`
      is 29.6 GiB and under it; `fuzz/`, `apps/wallet-gui/src-tauri/` and
      `apps/ledger-maya2c/` are separate workspaces holding 19.5 GiB with their own
      profiles — ADR-003

### Phase B — physical layout — **not started**

One mechanical commit, nothing else in it. ADR-001 has the reasoning.

- [ ] `git mv` into `crates/ bins/ apps/ hal/ sdks/ formal/ infra/`
- [ ] Rewrite every `path =` dependency
- [ ] Rewrite paths in `CLAUDE.md`, `.ignore`, `.claude/settings.json`, the workflows, `scripts/`, `infra/docker/Dockerfile*`, `infra/docker/docker-compose.yml`, `infra/k8s/`, `infra/deploy/`, `infra/terraform/`
- [ ] Acceptance: `cargo metadata` package set identical before and after, modulo `manifest_path`
- [ ] Acceptance: grep for every old path string across non-`target` files returns nothing
- [ ] Second cold rebuild (budget ~1 hour)

### Phase C — deterministic simulation harness — **complete, except the ports**

`sim/` (`maya-sim`). Dependency-free. Reasoning: ADR-006.

- [x] ADR-006: `madsim` vs `turmoil` vs hand-rolled, evaluated and decided
- [x] Seeded PRNG (SplitMix64, integer parts-per-million probabilities)
- [x] Virtual clock — integer nanoseconds, moves only when the scheduler moves it
- [x] Network model: latency bounds, loss, reordering, partitions that heal
- [x] Disk model: slow, corrupt, torn, lost-on-crash and full writes
- [x] Event queue with insertion-order tie-breaking, a deadline and an event budget
- [x] Every failure prints its seed, and replaying the seed reproduces the run
- [x] Per-model RNG streams, so adding a draw in one does not invalidate every seed
- [x] `crates/node/tests/chaos_simulator.rs` ported onto it — its private
      SplitMix64 is gone, `maya_sim::SimRng` is the one definition of the seed,
      13/13 still pass
- [x] `crates/node/tests/latency_sim_tests.rs` examined: it has **no
      randomness** and drives real libp2p nodes over in-memory transports, so
      there is no seed to unify and modelling it would replace a test of the
      real transport with a test of a model. Left alone deliberately
- [ ] Revisit `madsim` when a test needs the node's *real* tokio scheduling to be deterministic — ADR-006

### Known follow-ups opened by this phase

- [ ] `chacha20poly1305` 0.10 / 0.11 split — ADR-005
- [ ] `ed25519-dalek` 2.2 vs libp2p's 3.0 — must close before `THREAT_INTEL_ACTIVATION_HEIGHT` moves — ADR-005
- [x] `clippy::needless_range_loop` in `bins/neural-gas-trainer/src/network.rs:110` — fixed, weights verified unchanged
- [ ] 1,707 `clippy::pedantic` diagnostics (489 cast lints, 308 `doc_markdown`, 118 more casts). `warn`, not denied — ADR-003. Ratcheted by `scripts/lint_debt.sh`, so it cannot grow unnoticed
- [ ] 73 `clippy::expect_used` in `--lib --bins`. Not gated; `unwrap_used` is
- [ ] `proc-macro-error2 v2.0.1` future-incompatibility (transitive, via a proc-macro dependency)
- [ ] `wasm-pack` is not installed, so the packaged `.wasm` and its size and
      signing figures are still unmeasured — `docs/sdk.md`
- [ ] Raise the machine's commit limit, or lower `[profile.dev.package."*"]`
      to `opt-level = 2`, so `jobs` need not be pinned to 4 —
      `reports/02-layout.md` §1

---

## Master Prompts 2–10 — subsystem work

Not started as *phases*; much of the material exists already, built out of
order. `features.toml` is the authority for what each one would be extending
rather than starting.

| # | Phase | Already in the tree |
|---|---|---|
| 2 | Cryptography | ML-KEM-768, ML-DSA-65, SLH-DSA, HQC, Groth16, VRF, MPC custody — all SHIPPED |
| 3 | State engine | tx root, state root, undo journal, invariant guard, pruning — SHIPPED. Stateless verification RESEARCH |
| 4 | DAG consensus | Ethash-style DAG PoW SHIPPED. Narwhal/Tusk batches and elastic shards RESEARCH |
| 5 | WASM VM | Cranelift JIT, fuel metering, module cache — SHIPPED |
| 6 | DeFi / RWA | `dex` and `governance` SHIPPED. `rwa`, `identity`, `iso20022` RESEARCH |
| 7 | Multi-transport networking | libp2p + PQ Noise, peer guard SHIPPED. `ebpf-net`, `radio-transport` RESEARCH. Satellite, laser, CCSDS, subsea, neutrino PLANNED |
| 8 | Security tooling | fuzz targets, Kani, supply-chain gates, exploit replays SHIPPED. `offsec-sandbox`, `threat-intel` RESEARCH |
| 9 | Clients | Tauri wallet, Leptos explorer and dashboard, API gateway, SDKs, faucet — SHIPPED |
| 10 | Deployment | `infra/docker/Dockerfile`, `infra/docker/docker-compose.yml`, `infra/k8s/`, `infra/terraform/`, `infra/deploy/` exist and are **not** covered by `features.toml` or by any test |

### Master Prompt 2 — cryptography — **in progress**

Plan approved 2026-09-21. Brief-vs-tree findings (F1–F8) and decisions
D1–D4 are recorded in ADR-007 .. ADR-010; the new work extends existing
crates rather than duplicating them (D1).

- [x] P0 — ADR-007 (suites), ADR-008 (Plonky3), ADR-009 (KEMs), ADR-010 (entropy); vision rows; `features.toml` entries
- [x] P1 — suite registry `0x01/0x10/0x11/0x20/0x21/0x30` in `crypto-pq::suite`
- [x] P2 — NIST ACVP vectors vendored (`crates/crypto-pq/tests/vectors/acvp/`), ML-DSA/SLH-DSA/ML-KEM KATs pass
- [x] P1b — suite envelope + crypto-agility engine (policy, audit, migration, 1,000,000-account sim: 500 window blocks, 41 sweep blocks, no failed transfer)
- [ ] P3 — ML-KEM-1024, HQC-128/256 (draft) + KATs, DualKem, X-Wing + draft vectors
- [ ] P4 — secret hygiene sweep, dudect timing harness
- [ ] P5 — `hal/entropy`
- [ ] P6 — node: v2 envelope behind activation height, parity test, invariant 29 supersedes 4
- [ ] P7 — `crates/zk-stark` (Plonky3), `zk-privacy` rebuilt, arkworks/halo2 removed, `pqc_zk_tests.rs`
- [ ] P8 — custody: on-chain m-of-n, RESEARCH threshold feature, 3-of-5 with failures
- [ ] P9 — HTLC hash-locks (REAL) + Module-LWE (RESEARCH), `htlc_lattice_tests.rs`
- [ ] P10 — forward-secure archival keys, SLH-DSA seals, 100-epoch test
- [ ] P11 — Ledger app: DISPLAY_ADDRESS, RAM measurement, Speculos tests
- [ ] P12 — `benches/crypto.rs`, CSV/MD export, `reports/02-crypto.md`

---

## How to resume

1. `cargo xtask coverage` — the ledger, and whether every claim is still backed.
2. `cargo xtask disk` — artifact size against the 30 GiB ceiling, before starting a build.
3. `reports/01-foundation.md` — the last measured build and test numbers.
4. The first unticked box above.
