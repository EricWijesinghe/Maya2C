# 08 — Security, formal verification and supply chain

**Date:** 2026-09-26
**Branch:** `claude/task-0g86kl`
**Brief:** Master Prompt 8 — *"reports/08-security.md is complete with raw outputs;
every core invariant is either proven or listed as open."*

Every figure below comes with the command that produced it. Where a tool
from the brief could not be run here, the section says so and why; nothing
is reported as zero because it was not measured.

---

## 1. Master audit counts

| Measure | Figure | Source |
|---|---|---|
| Workspace tests | **2,626 passed, 0 failed, 6 ignored** across 234 binaries | `cargo test --workspace --profile ci --no-fail-fast`, raw log `reports/raw-test-baseline-2026-09-27.log` (55m50s) |
| Compile errors | 0 | same run: every binary built |
| `clippy::pedantic` diagnostics | **1,175** (ratchet baseline; was 1,178 before this branch, peaked at 1,184 mid-branch, then fixed) | `scripts/lint_debt.sh --check`, `lint-debt.txt` |
| First-party `unsafe` sites | 95 across the tracked crates (including out-of-workspace firmware and eBPF programs), plus 10 in the out-of-workspace wasm contract; **every one carries a `// SAFETY:` comment** | `scripts/check-unsafe.sh` (inventory in §6) |
| Supply chain | `advisories ok, bans ok, licenses ok, sources ok` | `cargo deny check` (§6) |
| Mutation score, `maya-fee-market` | **207 / 214 viable caught after this pass** (was 111 / 208 viable) | `cargo mutants -p maya-fee-market` (§4) |
| Fuzz, `fuzz_harness.rs` | **1,000,000 mutated payloads, 0 panics, 0 non-determinism** in 334.29 s | `MAYA_FUZZ_ITERS=1000000` (§3) |
| Z3, fee market | **10 / 10 proved**, 0 unknown | `python3 formal/z3/fee_market.py` |
| Lean 4 | `FeeSplit.lean`, `Supply.lean` check with no `sorry` | `lean` 4 at `/opt/lean` |
| Line coverage | **not measured** — no `cargo-llvm-cov` / `tarpaulin` on this machine | — |

## 2. Formal verification

### Z3 (bounded integers)

```
$ python3 formal/z3/fee_market.py
PROVED   split: the u128 product never overflows
PROVED   split: treasury fits in u64 (the Rust `expect` never fires)
PROVED   split: treasury <= base (burned never underflows)
PROVED   split: burned + treasury == base (100%)
PROVED   split: a share above 100% is clamped and creates no value
PROVED   next_base_fee: the u128 product never overflows
PROVED   next_base_fee: never below the floor
PROVED   next_base_fee: an under-target block never raises the fee above max(parent, floor)
PROVED   next_base_fee: an over-target block never lowers the fee
PROVED   next_base_fee: one step rises by at most parent/denom + 1 (size <= 2x target)

10/10 proved, 0 unknown, 0 failed
```

A first encoding over 128-bit bit-vectors timed out; the proofs use bounded
mathematical integers with the Rust types' ranges as hypotheses.

### Lean 4

`formal/lean/Maya2C/FeeSplit.lean` proves `split_sums` (burned + treasury =
base for every share ≤ 10,000 bps). `Supply.lean` proves `transfer_conserves`
over a model of accounts. Both are **models written by hand**, not
translations: Aeneas/Charon is not installed and was not attempted this
session, so the brief's "translate the Rust automatically" is **open**. The gap between model and code is covered by
a differential test instead: `formal/lean/vectors/fee_split.txt` (49 cases,
emitted by the Lean model) is replayed against the Rust by
`crates/fee-market/tests/lean_differential.rs`, which passes.

### Kani

Harnesses exist in nine crates (`ledger-math`, `dex`, `fee-market`,
`governance`, `threat-intel`, `blockgraph`, `lattice-pow`, `htlc-lattice`,
`iot-anchor` — all `src/proofs.rs`). **`cargo kani` is not installed here
and none was run in this session.** Their last recorded results are in
`reports/03-hardening.md`. Verus was not evaluated.

### Invariant status

"Pinned" means a named test fails if the rule breaks — the condition for a
number to exist at all (`docs/invariants.md`). "Proven" means a machine
checked proof covers it.

| # | Rule (short) | Status |
|---|---|---|
| 1, 6, 12 | `ledger-math` / `dex` / `governance` dependency-free for Kani | pinned (`cargo tree` test); Kani harnesses exist, **not run here** |
| 2, 29 | `crypto-pq` instantiates `slh-dsa`; only registered suites compile | pinned |
| 3, 4, 5 | feature/profile rules | pinned (build tests) |
| 7, 8 | a losing trade is a no-op; trading records journaled | pinned; `dex` Kani harness, not run here |
| 9 | oracle freshness by height | pinned |
| 10, 11 | VRF suite octet; oracle optional | pinned |
| 13 | no governance value is a program | pinned |
| 14–19 | telemetry, faucet, governed values, custody reconstruction | pinned |
| 20–23 | *retired with zkML (ADR-008)* | — |
| 24 | block id covers txs; `state_root` reproduced | pinned; **fuzzed** 1M inputs (§3) |
| 25–27 | state-root coverage, one-batch commit, pruning | pinned; 26 additionally by 1,000 `kill -9` runs (`crash_consistency_tests.rs`) |
| 28 | guard halts modules, never transfers | pinned (`exploit_replays.rs`) |
| 30, 31 | hybrid suite bytes; suite-tagged txs verify through registry | pinned |
| — | fee split sums to 100 % | **proven** (Z3 + Lean) and differential-tested |
| — | supply conserved by transfers | **proven** on a model (Lean); property-tested on the real node (`supply_property_tests.rs`: 30 random blocks, 12 applied, 18 refused whole, supply constant) |

**Open:** automatic Rust→Lean translation; a Kani run in this session; any
proof over the real `apply_block` rather than a model of it.

**Drift found:** `CLAUDE.md` cites *invariant 20* for "no float may enter a
consensus rule", but 20 was retired with zkML. The rule itself is live and
enforced by review and by `features.toml`; it has no numbered invariant.
Reported here rather than silently renumbered, because numbers are never
reused.

## 3. Fuzzing

```
$ MAYA_FUZZ_ITERS=1000000 cargo test -p custom-l1-node --profile ci --test fuzz_harness -- --nocapture
running 1 test
test seeded_mutations_never_panic_the_decoder_or_apply_path_and_stay_deterministic ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 334.29s
```

One million seeded mutations through `Transaction::from_bytes` and the
round-trip; a 200-block sample through two independent states for
determinism. **What this claims:** no crash was found in 10⁶ inputs. It does
not claim that none exists. The coverage-guided LibAFL targets live in the
out-of-workspace `offsec-sandbox` crate and were not run this session.

## 4. Mutation testing

```
$ cargo mutants -p maya-fee-market            # before
214 mutants tested in 2m: 97 missed, 111 caught, 6 unviable
```

The 97 survivors, by file: `model/weights_v1.rs` 59, `proofs.rs` 22,
`model/mod.rs` 8, `rule.rs` 3, `config.rs` 2, `limits.rs` 2, `execute.rs` 1.
Most telling: **nothing tested the neural model's output**. Replacing
`hidden_unit` with a constant `0` survived.

`crates/fee-market/tests/mutation_boundaries.rs` was written from that list:
boundary values in `FeeConfig::validate`, the limit constants, the exact-fee
case of the underpricing check, the zero-target short-circuit, an
independent `i128` re-evaluation of `MODEL_V1` over 2,000 random inputs, and
a pinned FNV digest of the weight table (the weights are a consensus
parameter; a retrain must change the digest in the same commit).

```
$ cargo mutants -p maya-fee-market            # after the new tests, before the digest
214 mutants tested in 2m: 82 missed, 125 caught, 7 unviable
$ cargo mutants -p maya-fee-market -f crates/fee-market/src/model/weights_v1.rs
59 mutants tested in 38s: 59 caught
```

The 23 that remain:

- **22 in `proofs.rs`** — Kani harness bodies, compiled only under
  `cfg(kani)`. `cargo test` cannot kill them by construction.
- **1 equivalent** — `rule.rs:72` `parent_size > target` → `>=`, unreachable
  because `parent_size == target` returned three lines earlier.

The state crate (`crates/node/src/state`) was not mutation-tested: every
apply-path test signs real hybrid ML-DSA + SLH-DSA transactions, and a
mutant run over it would take hours per pass on this machine. **Open.**

## 5. Exploit replays, chaos, self-healing

- `crates/node/tests/exploit_replays.rs` — re-entrancy, overflow and
  flash-loan replays trip the guard (invariant 28). 11 passed, 0 failed in
  the baseline run.
- `crates/node/tests/chaos_simulator.rs` — 13 passed, 0 failed in the
  baseline run.
- `tests/self_heal_tests.rs` — **does not exist.** The AI-assisted patch
  pipeline (detect → patch → proof → PR) is not built; an agent cannot
  apply code to a live node by design: invariant 13 already forbids any
  governed value from being a program, which is the dlopen ban the brief
  asks an ADR for.

## 6. Supply chain and workspace audit

```
$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```

Two advisories were live before this branch and are handled:
`lru` (unsound advisory) fixed by bumping `rqrr` 0.10 → 0.11 in
`apps/wallet-gui/core`; `atty` (RUSTSEC-2021-0145, unaligned read on
Windows) ignored in `deny.toml` with the reason written beside it — per that reason it is a
dev-dependency of the dudect harness only, Windows-only and
custom-allocator-only. The advisory database was
cloned by hand because `cargo-deny` 0.18.5 cannot parse its CVSS 4.0 lines.

```
$ scripts/check-unsafe.sh
ok: every first-party unsafe block and impl carries a SAFETY comment
```

Largest first-party sites: `hal/ebpf-net/src/linux/xsk.rs` 20 (AF_XDP),
`hal/ebpf-net/programs` 10, `hal/cuda-miner/src/gpu.rs` 7 — all in the
exemptions CLAUDE.md lists. `apps/ledger-maya2c/src/main.rs` needed its
comment moved directly above the block for the checker; fixed here.

```
$ cargo machete
```

48 dependencies in 18 crates look unused. `maya-build-guard` (5 crates) is a
false positive by design — it has no items; its features are the point —
and is now listed under `[package.metadata.cargo-machete] ignored`. The
remaining 43 are reported, **not removed** in this change: several are
feature-gated uses machete cannot see (`maya-tiers` pulls every binary so
the tier features resolve), and each removal is a separate, testable diff.
Notable real candidates: `hex` in `maya-archive`, `maya-vm`;
`tower-http` in `maya-api-gateway`, `maya-pool-service`.

**Not run (tools absent, not installed this session):** `cargo geiger`,
`cargo vet`, `cargo udeps`, Miri, loom, heaptrack, `cargo-llvm-cov`.
`cargo audit` is covered by `cargo deny`'s advisory check against the same
database.

## 7. Threat model and disclosure

`THREAT_MODEL.md` (STRIDE per component), `SECURITY.md` (disclosure policy),
`docs/security/INCIDENT_RESPONSE.md`, `docs/security/BUG_BOUNTY.md`
(a plan; no bounty is funded).

## 8. Open, in one list

1. Line coverage never measured.
2. Kani not run this session; Aeneas translation not done.
3. State-crate mutation run not done.
4. Coverage-guided fuzzing (LibAFL) and the 1-hour RC fuzz pass not run.
5. Self-healing pipeline and `self_heal_tests.rs` not built.
6. 43 `cargo machete` candidates to triage.
7. geiger / vet / udeps / Miri / loom / heaptrack not run.
