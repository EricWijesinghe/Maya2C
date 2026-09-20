# 04 — Every item of the foundation brief, and where each one landed

**Date:** 2026-09-20
**Branch:** `feat/complete-session-work`

`reports/00` is the audit, `01` phase A, `02` phases B and C, `03` the first
round of hardening. This one walks the brief line by line, because the
instruction was to leave nothing unfinished.

Two things were done differently from the literal text, and both are stated
where they occur rather than buried: the brief's own first line says *"Do NOT
build product features in this phase"*, so directories the brief names for
things this repository does not contain (`portal`, `dev-hub`, python and go
SDKs, Lean 4, helm, `maya2c-cli`) are created with a README defining what
belongs there; and `features.toml`'s register records prompts 161–162 as
**unallocated**, because `docs/trajectory.md` defines 160 and an invented
deliverable is worse than a recorded gap.

---

## 1. Audit

| Asked | Result |
|---|---|
| Map every crate, binary, test, bench, script; mark which compile | `reports/00-inventory.md` |
| Measure `target/`, then `cargo clean` | 316 GiB measured, cleaned |
| Find duplicates, plan merges, move superseded code to `attic/` | Three of four claimed duplications do not exist. `attic/README.md` records the finding and the rule for the next consolidation |

## 2. Workspace layout

| Asked | Result |
|---|---|
| `resolver = "3"` | Stated, not inferred |
| `edition = "2024"` | `[workspace.package]` |
| `[workspace.dependencies]`, one version per dependency | 41 entries, 450 declarations migrated |
| `unsafe_op_in_unsafe_fn = deny` | Done |
| `clippy::unwrap_used = deny` outside tests | **Denied in the manifest.** 354 test scopes carry an explicit inner `#![allow(...)]` — 142 test/bench/example files and 212 `#[cfg(test)]` modules. `--lib --bins` reports zero |
| `clippy::pedantic = warn` | Done, and ratcheted — `scripts/lint_debt.sh` |
| `crates/ bins/ apps/ hal/ sdks/ formal/ infra/ sim/ fuzz/ benches/ tests/ xtask/ docs/ reports/ attic/` | All present |
| Feature tiers `core` / `extended` / `frontier`; default build compiles core only | `crates/tiers`, a facade crate whose manifest *is* the artifact, plus `default-members`. All three tiers build |
| `rust-toolchain.toml` with rustfmt, clippy, miri, rust-src, llvm-tools | Done |

### The two that needed care

**`bins/`.** The brief names `maya2c-node`, `maya2c-miner`, `maya2c-gpu-miner`,
`l1-wallet`, `maya2c-cli`, `genesis-ceremony`. The node's five binaries were
`[[bin]]` targets of the node crate; they are now six packages:
`maya2c-node`, `maya2c-miner`, `maya2c-genesis`, `genesis-ceremony`,
`maya2c-peerid`, and `maya2c-gpu-miner` — the last split out of
`hal/wgpu-miner`, which keeps the device backend while `bins/` holds the thing
an operator runs. **The split needed no widening of the node's API**: every
binary used only public paths, which is why it was possible at all.
`maya2c-cli` does not exist; `bins/maya2c-cli/README.md` says what it would be
for and why `xtask` is deliberately not it.

**The tiers.** The brief's `extended` tier names "DeFi, RWA, identity,
bridges" — `crates/dex`, `crates/rwa`, `crates/identity`, `crates/iso20022`.
Every one writes records under the state root (invariant 25), so a cargo
feature that removed one would make two honest nodes compute different state
roots: a chain split from a build flag. `crates/tiers` therefore gates
*separate processes and artifacts*, never a crate the node links. The node is
whole in every tier. ADR-002.

## 3. Build speed and disk

| Asked | Result |
|---|---|
| `[profile.dev] debug = "line-tables-only"`, `split-debuginfo = "unpacked"`, `incremental = true` | All three. `split-debuginfo = "unpacked"` verified to work on MSVC |
| `[profile.dev.package."*"] opt-level = 3, debug = false` | Done |
| `[profile.release] lto = "fat"`, `codegen-units = 1`, `strip = true` | Done, as asked |
| `panic = "abort"` for binaries | `[profile.dist]`. Cargo applies `panic` per *profile*, not per target kind: in `release` it breaks `cargo test` and hits the `cdylib`s. `cargo build --profile dist --bins` is the shipped-binary build |
| `[profile.ci]` inherits dev, `debug = false` | Done |
| mold on Linux | Verified against mold 2.30.0 under WSL on this toolchain |
| lld on Windows | `lld-link.exe`. LLVM is on PATH here; the `line-tables-only` overrides stay and are independent of which linker runs |
| sccache if installed | Installed (0.18.0). **Not** set as `rustc-wrapper`: cargo fails outright when a configured wrapper is absent, so it would break every clone without it, and sccache does not cache incremental builds — with `incremental = true` the two defeat each other. `.cargo/config.toml` documents the per-invocation use where it pays, which is `--profile ci` |
| `cargo xtask disk`, warn above 30 GB | Done, and `--check` fails |

### The ceiling, measured against the brief's own criterion

The criterion is *"`target/` stays under 30 GB after a full core build"*.

```
$ cargo clean
     Removed 152741 files, 65.7GiB total
$ time cargo build          # default-members: the core tier
real    15m26.307s
$ cargo xtask disk --check
target                                  3.7 GiB
apps/ledger-maya2c/target             611.3 MiB
hal/ebpf-net/programs/target          548.1 MiB
docs/site/node_modules                201.6 MiB
contracts/token-swap/target             1.4 MiB
target-contracts                       13.8 KiB
---------------------------------- ------------
total                                   5.0 GiB
ceiling 30.0 GiB
--- disk exit 0 ---
```

**3.7 GiB, against a 30 GiB ceiling.** For contrast, the same tree with every
profile and tier built — dev, ci, release, and all three tier feature sets —
reached 67.0 GiB before that clean. The ceiling is a budget for *a* build, and
`--check` is what stops the accumulation that filled this volume twice.

`fuzz/` and `apps/wallet-gui/src-tauri/` no longer appear at all: their
profiles were fixed in `reports/03-hardening.md` §2 and their stale caches
reclaimed.

## 4. Reality ledger

164 `[[feature]]` entries — prompts 1–162 plus Benchmark and Memory — and 90
`[[subsystem]]` entries, the per-subsystem ledger with the tests that would
fail if each broke. `cargo xtask coverage` prints both and gates both.

`--verify-targets` additionally asks cargo-nextest whether each named test is
a target it would actually run. That check earned itself immediately: it found
three entries naming things cargo cannot execute — a directory rather than a
file, a Linux-and-root-only target, and a crate outside the workspace. All
three now name a `gate` saying what does run them.

```
features.toml: 164 register entries + 90 subsystems, all claims backed and
every named test is a target cargo-nextest can run.
```

## 5. Deterministic simulation harness

`sim/` (`maya-sim`), dependency-free, 47 tests. Covered in
`reports/02-layout.md` §2 and ADR-006.

**It is now used, which was the open item.**
`crates/node/tests/chaos_simulator.rs` had its own private nine-line
SplitMix64; that is gone and `maya_sim::SimRng` is the one definition of the
seed. All 13 of its scenarios still pass.

`crates/node/tests/latency_sim_tests.rs` was examined and **deliberately left
alone**: it contains no randomness at all — it sweeps a fixed latency list
over *real* libp2p nodes on in-memory transports. There is no seed to unify,
and replacing those nodes with a model would swap a test of the real transport
for a test of the model, which is precisely what ADR-006 says this harness
does not do.

## 6. Project memory

| Asked | Result |
|---|---|
| `CLAUDE.md` ≤ 300 lines | **300 exactly.** The invariants (197 lines), environment (171), workspace map and build detail (105) live in their own linked files |
| `docs/adr/`, ADR-001 now | ADR-001 through ADR-006 |
| `PROGRESS.md` | Master Prompts 1–10 with sub-steps |
| Standing Orders in `CLAUDE.md` | Nine, numbered |

## 7. CI

`ci.yml`: `ledger`, `fmt`, `clippy`, `test`, `deny`. The test job runs the
**core tier first** and then the whole workspace — a strict subset sharing
every compiled unit, so it costs the cache and not a second build — then
verifies the ledger's targets, then reports artifact size.

`cargo-deny` is in `ci.yml` *and* in `security-audit.yml`. That overlap is
deliberate: `security-audit.yml` is scheduled and path-filtered, so a pull
request touching only `Cargo.toml` would otherwise reach `master` without a
licence check. The job builds nothing and takes seconds.

`nightly.yml`: `frontier` cross targets, `fuzz`, `kani`, `coverage` floors,
and the `lint-debt` ratchet.

## 8. Done when

| Criterion | Result |
|---|---|
| `cargo check --workspace` passes for core | Yes |
| clippy clean for core | Yes — the denied set is clean; `pedantic` is `warn` and ratcheted at 1,205 |
| `target/` under 30 GB after a full core build | **3.7 GiB** |
| `features.toml` lists all 164 entries | **164**, plus 90 subsystems |
| `CLAUDE.md`, `PROGRESS.md`, ADR-001 exist | Yes |
| `reports/01-foundation.md` contains real command output | Yes, and 00, 02, 03, 04 |

## 9. Final verification

```
cargo nextest run --workspace     2,516 tests / 170 binaries, 2,516 passed, 6 skipped
cargo fmt --all --check           clean
clippy --lib --bins -D unwrap_used                       0
clippy --all-targets -A pedantic -A unwrap_used -D warnings   0
cargo deny check                  advisories ok, bans ok, licenses ok, sources ok
cargo xtask coverage --verify-targets   164 + 90, all backed
scripts/lint_debt.sh              1,205 (was 1,707)
cargo xtask disk --check          5.0 GiB / 30 GiB, exit 0
cargo build -p maya-tiers                       core     ok
cargo build -p maya-tiers --features extended   ok
cargo build -p maya-tiers --features frontier   ok
```

```
$ bash scripts/doc_coverage.sh --check
TOTAL                          6583     6564      99%
doc_coverage: building the workspace docs with broken intra-doc links denied
--- doc exit 0 ---
```

## 10. One thing this report nearly missed

The check that confirms no stale binary name survived the `bins/` split did
not run when it was supposed to. It was invoked as

```
rg -n 'src/bin/node|--bin node|...' --glob '!target' | head -8
```

with **no path argument**, and stdin attached to a pipe — so ripgrep searched
stdin and blocked. It sat there for sixteen hours looking like a running
build, and its result was never reported.

When it was finally killed and the search run properly, it found one thing
that was actually broken:

```
scripts/local_cluster.sh:84
-  cargo build $CARGO_FLAGS --bin node --bin genesis --bin genesis-ceremony
+  cargo build $CARGO_FLAGS --bin maya2c-node --bin maya2c-genesis --bin genesis-ceremony
```

plus 21 files of stale prose pointing at `src/bin/node.rs` and
`crates/node/src/bin/genesis-ceremony.rs`. All fixed.

Two lessons, both cheap:

- **`rg` without a path reads stdin.** Always give it a directory.
- **A command whose output nobody reads is a check that did not happen.** This
  one was reported as "still running" for sixteen hours and I took that at
  face value twice.

`--bin genesis-ceremony` is *not* stale, incidentally: that binary kept its
name through the split.

## 11. The one thing still open

**No workflow has run on a GitHub runner.** Every gate above was executed
locally on Windows and made to pass, including the two that would have failed
on the first push. What remains unexercised is the runner itself: `mold`, the
`apt` steps, the action versions, the disk budget, and Linux-specific
behaviour of the `lld-link`-free path.

That cannot be settled without pushing, and it is the only item in this report
whose status is a prediction rather than a measurement.
