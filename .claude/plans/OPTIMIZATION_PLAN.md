# Maya2C Optimization & Industry-Standard Enhancement Plan

Generated from analysis 2026-10-09. Prioritized by P-band (Operating Protocol).

---

## P0 — Build Broken / Tests Failing

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| 1 | `dag-bft` property tests don't compile | `crates/dag-bft/tests/property_tests.rs:6,13` | Fix `proptest!` macro syntax |
| 2 | `unimplemented!()` in ML-DSA keygen | `crates/node/src/crypto/keys.rs:297,301` | Implement or gate behind feature |
| 3 | `panic!()` on invalid KEM id | `crates/crypto-pq/src/kem_suite/dual.rs:59` | Return `Result` instead |

---

## P1 — Critical Gaps from Gap Register

| # | Gap | Target | Evidence Required |
|---|-----|--------|-------------------|
| 1 | Core line coverage never measured | `cargo llvm-cov` on `custom-l1-node`, `maya-dag-bft`, `maya-vm`, `maya-fee-market` | HTML + LCOV reports |
| 2 | Property tests for consensus | `dag-bft` - add `quorum`, `validity`, `weighted_majority` proofs | Seeded proptest, CI gate |
| 3 | Property tests for VM | `vm` - add fuel determinism, memory bounds | Seeded proptest, CI gate |
| 4 | Property tests for fees | `fee-market` - already exist, need CI enforcement | Verify they run in CI |

---

## P2 — Making Claimed-Working Features Verified

### Performance Optimizations (Measured Impact)

| # | Bottleneck | Location | Fix | Expected Gain |
|---|------------|----------|-----|---------------|
| 1 | Quadratic overlay clone in `select_applicable` | `crates/node/src/state/db.rs:1068-1076` | Copy-on-write overlay or batch staging | 10-50x block build throughput |
| 2 | N+1 RocksDB reads in `select_applicable` | `crates/node/src/state/db.rs:1048-1077` | Batch verification + staged reads | 5-10x verification speed |
| 3 | Linear ancestor walk in reorg | `crates/node/src/consensus/chain.rs:540-559` | Binary lifting / skip list for ancestors | O(log n) reorg |
| 4 | Unsharded `Mutex<HashMap>` mempool | `crates/node/src/network/mempool.rs:33` | Sharded `DashMap` or `tokio::sync::RwLock` | Eliminate lock contention |
| 5 | Global `Mutex<HashMap>` rate limiter | `crates/node/src/rpc/limit.rs:115-160` | Per-IP token bucket (lock-free) | Handle 10k+ req/s |
| 6 | Manual LRU cache in RPC | `crates/node/src/rpc/bootstrap.rs:75-93`, `market_http.rs:86-135` | Use `moka` crate | O(1) cache, eviction |
| 7 | Single `Mutex` for all epoch caches | `crates/node/src/crypto/dag/registry.rs:127` | Per-epoch `RwLock` or lock-free | Parallel mining/validation |
| 8 | 3x BTreeMap redundancy in DAG | `crates/dag-bft/src/dag.rs:15-24` | Single indexed structure | 66% memory reduction |

### Code Quality Fixes

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| 1 | `db.rs` >2000 lines | `crates/node/src/state/db.rs` | Split into `overlay.rs`, `apply.rs`, `verify.rs`, `prune.rs`, `blocks.rs` |
| 2 | `chain.rs` ~720 lines | `crates/node/src/consensus/chain.rs` | Extract `fork_choice.rs`, `reorg.rs`, `candidate.rs` |
| 3 | `driver.rs` ~900 lines | `crates/node/src/consensus/bft/driver.rs` | Extract `round_manager.rs`, `certificate_store.rs` |
| 4 | `dex_exec.rs` >800 lines | `crates/node/src/state/dex_exec.rs` | Split by AMM/book/batch |
| 5 | 72 clippy warnings in node | `cargo clippy -p custom-l1-node` | Fix `needless_pass_by_value`, `cast_possible_truncation` |
| 6 | 37 warnings in VM | `cargo clippy -p maya-vm` | Fix `cast_sign_loss`, `cast_possible_truncation` |
| 7 | 20 warnings in fee-market | `cargo clippy -p maya-fee-market` | Fix `cast_sign_loss`, `cast_possible_wrap` |

### Security Hardening

| # | Issue | Location | Fix |
|---|-------|----------|-----|
| 1 | `unwrap_or_else` on EnvFilter panic | `crates/api-gateway/src/main.rs:75` | Parse with `?`, default filter |
| 2 | Implicit address format assumption | `crates/api-gateway/src/rest.rs:134` | Explicit `0x` handling + validation |
| 3 | `Mutex<Option<T>>` mining threads | `crates/node/src/consensus/miner.rs:149-200` | Bounded channel + backpressure |
| 4 | Missing SAFETY comment | `crates/multivm/src/sbf.rs:91` | Document invariants |
| 5 | `allow(unwrap_used)` in prod-adjacent | Multiple test files | Move to test-only modules |

### Build/Artifact Size Reduction

| # | Issue | Fix |
|---|-------|-----|
| 1 | 76.4 GiB artifacts vs 30 GiB ceiling | `cargo xtask sweep --clean` before builds; add `--timings` to CI |
| 2 | Nested workspace targets not cleaned | Add `cargo clean -p wallet-gui -p ledger-maya2c -p fuzz` to `xtask clean` |
| 3 | Debug info in dev profile | Profile overrides already exist (ADR-003); verify `[profile.dev.package."*"]` applied |
| 4 | `jobs = 4` load-bearing | Raise commit limit or lower opt-level (see PROGRESS.md §1 Phase A) |

---

## P3 — Current Milestone (M1 CORE SOLID) Remaining

| # | Criterion | Status | Work |
|---|-----------|--------|------|
| 1 | Clean build | ✅ Met | — |
| 2 | Tests pass | ✅ Met | — |
| 3 | Gates (fmt/clippy/unsafe/ratchet/doc) | ✅ Met | — |
| 4 | CI green on master | ⚠️ Pending | Fix dag-bft compile error |
| 5 | Property tests (fees/consensus/VM) | ❌ Open | dag-bft, VM need property tests |
| 6 | Fuzzing all decoders | ✅ Met | 16 targets |
| 7 | Kani proofs | ✅ Met | PR #1 green |
| 8 | Line coverage measured | ❌ Open | Run `cargo llvm-cov` |
| 9 | MP12 baseline recorded | ✅ Met | `reports/12-baseline.md` |
| 10 | MP06 core parts | 👤 Owner | Q1 in EMPIRE.md |

---

## P4 — Adoption-Critical (M3 USABLE)

| # | Area | Blockers |
|---|------|----------|
| 1 | Wallet UX (MP22, MP29) | Usability studies with outside participants |
| 2 | Safe contracts (MP23) | Opt-in → mandatory for consensus imports |
| 3 | Developer platform (MP24) | Developer study; MCP server hardening |
| 4 | Explorer UX (MP29) | Screenshot diffs in CI; plain language |

---

## P5 — Differentiators (M4 CONNECTED)

| # | Feature | Status |
|---|---------|--------|
| 1 | PQ weight (MP13) | ✅ Met |
| 2 | Interop primitives (MP25) | ZK light client RESEARCH |
| 3 | Privacy primitive (MP27) | Built; activation post-M7 |
| 4 | Quantum harbor (MP28) | Partial |

---

## P6 — Deferred / Frontier

**Do not touch until milestone.** (Per Operating Protocol)

---

## Implementation Order

### Week 1 (P0 + P1 Critical)
1. Fix `dag-bft` property test compilation
2. Implement ML-DSA keygen or gate it
3. Run `cargo llvm-cov` on core 4 crates; publish reports
4. Add `cargo llvm-cov --fail-under-lines 80` to CI for core crates

### Week 2 (P2 Performance)
1. Fix quadratic overlay clone in `select_applicable`
2. Batch verification in `select_applicable`
3. Replace global `Mutex<HashMap>` with `moka` cache + token bucket
4. Add binary lifting to `common_ancestor`

### Week 3 (P2 Code Quality + Security)
1. Split `db.rs`, `chain.rs`, `driver.rs`, `dex_exec.rs`
2. Fix all clippy warnings in core crates
3. Add SAFETY comments to `multivm/src/sbf.rs`
4. Remove `allow(unwrap_used)` from non-test code

### Week 4 (P2 Artifact Size + CI)
1. Extend `cargo xtask clean` to nested workspaces
2. Add per-PR doc link check (BACKLOG.md item)
3. Add property test CI gate for dag-bft, vm, fee-market

---

## Metrics to Track

| Metric | Current | Target | Measurement |
|--------|---------|--------|-------------|
| Build artifact size | 76.4 GiB | <30 GiB | `cargo xtask disk` |
| Core line coverage | Unknown | ≥80% | `cargo llvm-cov` |
| Clippy warnings (node) | 72 | 0 | `cargo clippy -p custom-l1-node` |
| Clippy warnings (vm) | 37 | 0 | `cargo clippy -p maya-vm` |
| Clippy warnings (fee-market) | 20 | 0 | `cargo clippy -p maya-fee-market` |
| Property test coverage | 1/3 crates | 3/3 | CI gate |
| `select_applicable` throughput | ~900 tx/block | >5000 tx/block | `reports/12-performance.md` |
| Reorg time (100 blocks) | O(n) | O(log n) | Benchmark |
| RPC rate limit throughput | ~100 req/s | >10k req/s | Load test |

---

## Verification Gates (Before Marking DONE)

Per Operating Protocol "Proof discipline":
- [ ] `cargo check` passes on affected crates
- [ ] `cargo test -p <crate>` passes (no new failures)
- [ ] Real command output in `reports/`
- [ ] `features.toml` updated if subsystem status changes
- [ ] Commit message says *why*
- [ ] `PROGRESS.md` ticked
- [ ] Self-audit: re-run checks and paste output

---

## Files to Create/Update

| File | Purpose |
|------|---------|
| `reports/optimization-baseline.md` | Before/after measurements |
| `reports/coverage-<crate>.md` | Per-crate coverage reports |
| `scripts/coverage_gate.sh` | CI integration for `llvm-cov` |
| `scripts/perf_bench.rs` | Microbenchmarks for hot paths |

---

## Notes

- **Never hard-code benchmark targets** (Standing Order 2). Measure and report.
- **Work in small steps** (Standing Order 7). `cargo check` → test → commit → tick.
- **No invented numbers** (Proof discipline 5). Missing = "MISSING".
- **Consensus-critical = no floats, no HashMap iteration, no wall-clock, no unsafe without SAFETY** (Standing Order 4).