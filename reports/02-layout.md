# 02 — Phase B (layout) and Phase C (simulation), measured

**Branch:** `feat/complete-session-work`
**Date:** 2026-09-20
**Toolchain:** `nightly-2026-07-15-x86_64-pc-windows-msvc`

`reports/00-inventory.md` is the before-state; `reports/01-foundation.md` is
phase A. Paths quoted in those two are pre-move and are left alone — they are
pasted command output, and rewriting them would make them say something the
command did not.

---

## 1. Phase B — the move

45 directories moved. The root package `custom-l1-node` became `crates/node`
and the root manifest became a virtual workspace.

| Directory | Holds | Members |
|---|---|---|
| `crates/` | libraries — anything another crate links | 28 |
| `bins/` | packages whose product is an executable and which nothing else links | 6 |
| `apps/` | things a person opens | 5 (2 members) |
| `hal/` | hardware, and the simulators standing in for it | 6 (4 members) |
| `sdks/` | bindings for other languages | 3 (2 members) |
| `sim/` | the deterministic simulation harness (phase C) | 1 |
| `xtask/` | workspace automation | 1 |
| `formal/` | no crates; a README saying where the proofs actually are | — |
| `infra/` | terraform, ansible, k8s, deploy | — |

The repository root went from **90 entries to 36**.

### Acceptance test

ADR-001 set one: the package set must be identical.

```
$ python - <<< 'compare cargo metadata --no-deps before and after'
before: 45  after: 46
added  : ['xtask']
removed: []

PACKAGE SET PRESERVED (+xtask, added after the baseline)
```

Nothing was lost, renamed or duplicated. 75 `path =` dependencies were checked
and every one resolves to a manifest that exists — the checker refuses to
rewrite a path it cannot verify, and reported none unresolved.

### Deviations from the brief's layout

Three, each because the alternative is a code change rather than a move, and
each recorded in ADR-001:

- **The node's five binaries stay in `crates/node/src/bin/`.** The brief put
  `maya2c-node`, `maya2c-miner` and `genesis-ceremony` under `bins/`. They are
  `[[bin]]` targets of one library crate, not packages; splitting them means
  new crates and new module boundaries.
- **`formal/` has no crates.** Kani proofs are `#[cfg(kani)]` modules inside
  the crates they verify, and that placement is load-bearing: Kani compiles a
  crate with its whole dependency graph, which is *why* those crates are
  dependency-free (invariants 1, 6, 12). A proof crate here would depend on
  the crate it verifies and put the dependency back. `formal/README.md` says
  so, and says Lean 4 is PLANNED and Aeneas unevaluated.
- **Package names did not change.** Directories moved; `maya-sdk-ffi` is still
  `maya-sdk-ffi` under `sdks/sdk-ffi`. The one rename is
  `app-maya2c` → `apps/ledger-maya2c`, which the brief named explicitly.

### What a package-set check cannot catch

Five places computed repository-relative paths by walking up a fixed number of
levels from `CARGO_MANIFEST_DIR`. All five were silently wrong after the move —
they compile, and then read the wrong directory:

| Site | Was | Now |
|---|---|---|
| `crates/zkml-prover/tests/proof_tests.rs` | `../tests/fixtures/zkml` | `../node/tests/fixtures/zkml` |
| `crates/zkml-prover/tests/circuit_tests.rs` | same | same |
| `crates/zkml-prover/benches/verify.rs` | same | same |
| `bins/neural-gas-trainer/src/main.rs` | `../fee-market/src/model/weights_v1.rs` | `../../crates/fee-market/…` |
| `crates/vm/tests/token_swap_tests.rs` | `.parent()` | `.parent().and_then(parent)` |
| `hal/cuda-miner/tests/dag_parity.rs` | `../tests/fixtures/dag_vectors.json` | `../../crates/node/tests/fixtures/…` |

The last one was **not** found by reading; it was found by the test suite,
which failed two `dag_parity` cases with
`NotFound: The system cannot find the path specified`. Five of the six were
caught by grepping for `CARGO_MANIFEST_DIR`; that one used `concat!` with a
string literal instead of `.join()`, so the grep's follow-up lines missed it.
A package-set check cannot catch any of them, and neither can the compiler.

The trainer one is the interesting one: it is the path the generated-weights
drift check reads, and a wrong path there would have made the check compare
against nothing.

```
$ cargo run -q -p maya-neural-gas-trainer -- --check
D:\Maya2C\bins\neural-gas-trainer\../../crates/fee-market/src/model/weights_v1.rs matches a fresh run
```

### Reference sweep

184 files and 1,105 lines outside the manifests, plus 47 comment lines inside
them. The rules were deliberately narrow:

- `old/` as a path segment is rewritten everywhere.
- A **bare** directory name in quotes is rewritten only in config files
  (`.gitignore`, `.ignore`, `.claude/settings.json`, the workflows,
  `docker-compose.yml`, `xtask/src/disk.rs`). A first attempt without that
  restriction turned `Vm::new().expect("vm")` into
  `.expect("crates/vm")` in 33 places — package names did not change, so a
  bare name in Rust or prose is left alone.
- **`reports/` and `docs/adr/` were reverted after the sweep touched them.**
  They contain pasted command output. Rewriting `wallet-gui/src-tauri/target`
  to `apps/wallet-gui/src-tauri/target` inside a quoted `cargo xtask disk`
  block would be editing a record of what a command printed, which Standing
  Order 1 exists to prevent.

### The failure that wasted the most time, and what it actually was

Several `cargo build --workspace --all-targets` and `cargo nextest run
--workspace` runs failed, reproducibly, with

```
error: crate `wasmtime_internal_cranelift` required to be available in rlib format, but was not found in this form
error[E0463]: can't find crate for `custom_l1_node`
```

cascading across a dozen crates, while the same crates built fine one at a
time. I proposed three explanations and **all three were wrong**:

1. stale metadata from running `cargo check`/`clippy` before `cargo build` —
   ruled out, it recurred immediately after `cargo clean`;
2. interleaving `-p` scopes with `--workspace` scopes — ruled out, it recurred
   with nothing else touching the target directory;
3. `--all-targets` and `cargo test --no-run` being different feature scopes —
   ruled out, `cargo clean` followed by nextest alone failed the same way.

Each of those is a real cargo phenomenon. None of them was this. The actual
cause only appeared once the output was read unfiltered instead of grepped for
`^error`:

```
error[E0786]: found invalid metadata files for crate `custom_l1_node`
  --> apps\faucet\src\main.rs:20:5
   = note: failed to mmap file 'D:\Maya2C\target\debug\deps\libcustom_l1_node-5dd154c0177f8ba2.rlib':
           The paging file is too small for this operation to complete. (os error 1455)
```

**`os error 1455` is Windows commit-charge exhaustion**, and the `E0463` /
"required to be available in rlib format" lines are what *other* jobs report
when that one fails. Measured while it was happening:

```
TotalVisibleMemGB : 31.4
CommitLimitGB     : 53.6
CommitFreeGB      : 12.9
PageFileGB        : 22.23
```

Two dozen `rustc` processes compiling wasmtime, arkworks and halo2 at
`opt-level = 3` — which `[profile.dev.package."*"]` now applies to *every*
dependency rather than the thirty that were named before — each mmapping a
large rlib, go past a 53.6 GB commit limit.

Three things follow.

- **It is not related to the move.** The same failure is reachable before it;
  the move only changed how often the machine was pushed that hard.
- **`CARGO_BUILD_JOBS` is load-bearing on this machine**, and
  `reports/01-foundation.md` §8 originally said the opposite on the strength of
  one successful run. That is corrected there. `line-tables-only` fixed the
  *linker's* memory problem (`LNK1102`); the *compiler's* remains.
- **Grepping build output for `^error` hides the cause.** Every wrong
  explanation above came from reading a filtered cascade. The `E0786` line
  that names the real problem is indented under a `note:`.

### Build and test, after the move

`CARGO_BUILD_JOBS` is pinned to 4 in `.cargo/config.toml` for the reason
above.

```
$ cargo build --workspace            # libraries and binaries
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 57.27s

$ time cargo nextest run --workspace --no-fail-fast --status-level fail
    Starting 2515 tests across 169 binaries (6 tests skipped)
     Summary [ 480.367s] 2515 tests run: 2515 passed (39 slow), 6 skipped
real    8m8.000s
```

**2,515 tests across 169 binaries. 2,515 passed, 0 failed, 6 skipped.**

That is 47 more tests and 2 more binaries than the pre-move run
(`reports/01-foundation.md` §8: 2,468 across 167). The difference is `sim`:
its unit tests and `sim/tests/determinism.rs`. Nothing that passed before the
move fails after it.

```
$ cargo xtask disk
target                                 18.1 GiB
fuzz/target                            10.2 GiB
apps/wallet-gui/src-tauri/target        8.7 GiB
apps/ledger-maya2c/target             611.3 MiB
hal/ebpf-net/programs/target          548.1 MiB
apps/wallet-gui/ui/target             255.4 MiB
docs/site/node_modules                201.6 MiB
contracts/token-swap/target             1.4 MiB
target-contracts                       13.8 KiB
---------------------------------- ------------
total                                  38.6 GiB
ceiling 30.0 GiB
free on this volume: 358.1 GiB
```

The root `target/` is 18.1 GiB here rather than the 29.6 GiB in
`reports/01-foundation.md` §1 because this build is `cargo nextest` only —
libraries, binaries and tests, no benches or examples. The total is over the
ceiling for the reason already recorded: 19.5 GiB of it belongs to nested
workspaces a root `cargo clean` never touches.

---

## 2. Phase C — `sim/`

A new dependency-free crate, `maya-sim`. 1,670 lines including tests.

| File | Lines | What it decides |
|---|---:|---|
| `sim/src/rng.rs` | 171 | every random choice, from one seed |
| `sim/src/clock.rs` | 197 | virtual time |
| `sim/src/net.rs` | 291 | latency, loss, reordering, partitions |
| `sim/src/disk.rs` | 290 | slow, corrupt, torn, lost-on-crash, full writes |
| `sim/src/world.rs` | 366 | the event queue |
| `sim/src/lib.rs` | 142 | `replay`, `next_seed` |
| `sim/tests/determinism.rs` | 213 | the property the crate exists for |

Three decisions are load-bearing, and ADR-006 records them:

**No dependencies.** A dependency is a second source of decisions — its own
RNG, its own hashing, its own iteration order — and any one of them makes a
replay a different run. The same rule that keeps `ledger-math` and `dex`
dependency-free for Kani keeps this one dependency-free for reproducibility.

**Per-model RNG streams.** `World::new` forks the root seed into a network
stream, a disk stream and a stream for the test's own draws. Adding a draw in
one does not shift the others. Without this, every recorded seed silently
changes meaning the next time somebody adds an `rng` call — which is how teams
stop trusting seeds. `a_forked_stream_does_not_disturb_the_models` pins it.

**Ties break on a sequence number.** Two events at the same virtual nanosecond
come out in insertion order on every machine. A heap keyed only on time is
free to reorder them, and a replay that reorders is not a replay.

`madsim` was evaluated and not taken. It is the stronger option — it replaces
`tokio`, so the node's *real* async code becomes deterministic rather than
code written against a model — and the reason to revisit it is exactly that.
It was rejected here because it takes ownership of the runtime across a
45-member workspace (and does not intercept libp2p), it is a dependency with
its own RNG inside the determinism boundary, and it would land inside a phase
whose subject is workspace layout. `turmoil` models the network only, and the
disk faults are half the value: invariants 25, 26 and 27 all rest on
"written, therefore durable".

### Not done

The existing simulation tests are **not** ported onto it.
`crates/node/tests/chaos_simulator.rs` and
`crates/node/tests/latency_sim_tests.rs` work today; porting them changes test
behaviour and belongs in a commit whose subject is that. Tracked in
`PROGRESS.md`.

### Verified

```
$ cargo nextest run -p maya-sim
```

`maya-sim`'s tests are part of the 2,515 above. They assert the properties the
crate exists for rather than its surface:

- the same seed reproduces a whole multi-node run — deliveries, tips, write
  outcomes, drop counts and the stop reason, over latency, 2% loss, 3%
  reordering and a failing disk;
- different seeds do not;
- a partition replays identically *and* changes the outcome, so a test cannot
  pass by silently not applying it;
- a test drawing its own randomness does not disturb what the network and the
  disk decide;
- a livelock exhausts the event budget instead of hanging;
- events at the same virtual nanosecond keep insertion order;
- a torn write is always short and never empty, and a one-byte write cannot
  tear.

One of these was rewritten before it ever ran. `disk_faults_are_part_of_the_replay`
asserted that at least one write in a run was faulty, against a model whose
fault rate made zero faults in ~84 writes perfectly legal — flaky by
construction. It now asks a model set to fail three times in four, and checks
replay separately.
