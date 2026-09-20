# 01 — Foundation phase, measured

**Branch:** `feat/complete-session-work`
**Range:** `223fe56` … (this phase)
**Date:** 2026-09-20
**Toolchain:** `nightly-2026-07-15-x86_64-pc-windows-msvc` — `rustc 1.99.0-nightly (da80ed070 2026-07-14)`
**Host:** Windows 11 Pro 10.0.29671, 24 logical CPUs, 32 GB RAM, D: volume

Every number here is pasted from the command that produced it. Where a target
was not met, the section says so rather than moving the target.

`reports/00-inventory.md` is the before-state this is measured against.

---

## 1. Disk

The foundation brief set a 30 GiB ceiling. `cargo xtask disk` is what measures
it, and `--check` is a CI step.

### Before

```
$ cargo xtask disk
Build artifacts
path                                       size
---------------------------------- ------------
target                                316.0 GiB
fuzz/target                            10.2 GiB
wallet-gui/src-tauri/target             8.7 GiB
app-maya2c/target                     611.3 MiB
ebpf-net/programs/target              548.1 MiB
wallet-gui/ui/target                  255.4 MiB
docs/site/node_modules                201.6 MiB
contracts/token-swap/target             1.4 MiB
target-contracts                       13.8 KiB
---------------------------------- ------------
total                                 336.5 GiB

ceiling 30.0 GiB  (9 measured in 5.0s)
free on this volume: 85.6 GiB

WARNING: build artifacts are 336.5 GiB , over the 30.0 GiB ceiling.
```

CLAUDE.md had said 356 GB and the session memory 415 GB. Both were true once;
neither was current. 336.5 GiB is.

### After `cargo clean`

```
$ time cargo clean
     Removed 188085 files, 317.4GiB total
real    1m28.064s
```

```
$ cargo xtask disk
target                                 71.7 MiB
fuzz/target                            10.2 GiB
wallet-gui/src-tauri/target             8.7 GiB
...
total                                  20.6 GiB
ceiling 30.0 GiB  (9 measured in 1.0s)
free on this volume: 377.3 GiB
```

`cargo clean` frees the root `target/` only. `fuzz/target` (10.2 GiB) and
`wallet-gui/src-tauri/target` (8.7 GiB) are separate workspaces with their own
build directories and survive it — 18.9 GiB of the 20.6 GiB remaining is those
two.

### After a full rebuild and the whole test suite

```
$ cargo xtask disk
target                                 29.6 GiB
fuzz/target                            10.2 GiB
wallet-gui/src-tauri/target             8.7 GiB
app-maya2c/target                     611.3 MiB
ebpf-net/programs/target              548.1 MiB
wallet-gui/ui/target                  255.4 MiB
docs/site/node_modules                201.6 MiB
contracts/token-swap/target             1.4 MiB
target-contracts                       13.8 KiB
---------------------------------- ------------
total                                  50.2 GiB
ceiling 30.0 GiB
free on this volume: 350.6 GiB
```

That is after `cargo build --workspace --all-targets` **and**
`cargo nextest run --workspace` — every binary, every test, every bench
target, linked and run.

**The root `target/` is 29.6 GiB, down from 316.0 GiB — a 10.7× reduction,
and under the 30 GiB ceiling.** That is what the profile change bought:
`debug = "line-tables-only"` workspace-wide plus
`[profile.dev.package."*"] debug = false`.

**The total across all nine artifact directories is 50.2 GiB, which is over.**
The 20.6 GiB that survives a root `cargo clean` is not something the dev
profile can reach: `fuzz/`, `wallet-gui/src-tauri/` and `app-maya2c/` are
separate workspaces with their own profiles, and `docs/site/node_modules` is
not Rust at all. Bringing the total under 30 GiB means giving those workspaces
the same profile treatment — which is real work, not a setting, and is not
part of this phase.

So the ceiling is met for the thing the brief was pointing at (the `target/`
folder that filled the volume) and not met for the sum. `cargo xtask disk`
reports the sum, because the volume fills on the sum.

## 2. Toolchain

Pinned in-tree for the first time (`rust-toolchain.toml`). The pin matches
what was already in use, so it triggered no rebuild:

```
$ time cargo check -p maya-ledger-math
info: component rust-std for target riscv32imc-unknown-none-elf is up to date
info: component rust-std for target thumbv8m.main-none-eabihf is up to date
info: component rust-std for target wasm32-unknown-unknown is up to date
info: component rustfmt is up to date
info: downloading 2 components
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.58s
real    0m15.055s
```

The two downloaded components are `miri` and `llvm-tools`, which the documented
workflows need (`cargo llvm-cov` for the 80% floor) and which were not
installed on this machine.

## 3. Dependencies

Before: 95 external dependencies across 45 manifests, no
`[workspace.dependencies]`, six declared at incompatible versions.

After: 40 entries in the workspace table, **450 declarations migrated** across
45 manifests, three splits closed and three left open with reasons recorded in
[ADR-005](../docs/adr/ADR-005-dependency-unification.md).

The `sha3` unification was the one with consequences — three Kani-verified
crates were hashing with two releases of the same SHAKE. Verified before it
was committed:

```
$ cargo test -p maya-iot-anchor
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo build -p maya-iot-anchor --target thumbv8m.main-none-eabihf --no-default-features
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.46s
$ cargo build -p maya-iot-anchor --target riscv32imc-unknown-none-elf --no-default-features
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.61s
```

**Transitive duplication did not move.** `cargo tree -d --workspace` reported
110 duplicate version groups before and 110 after. `sha3` still resolves at
three majors because 0.10 arrives through `fips204` and 0.12 through
`hqc-kem`; no first-party declaration can remove them. The
`curve25519-dalek` / `ed25519-dalek` finding from `00-inventory.md` §7 is
unchanged and is now tracked as ADR-005's "revisit when".

## 4. Build, cold, with the new profile and lints

`cargo clean` first, so this is a genuine cold build with
`[profile.dev] debug = "line-tables-only"` and
`[profile.dev.package."*"] opt-level = 3, debug = false` in force.

```
$ time cargo check --workspace --all-targets --message-format short
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 34s
warning: the following packages contain code that will be rejected by a future
version of Rust: proc-macro-error2 v2.0.1
real    4m35.070s
```

**Zero errors and zero rustc warnings**, with `unsafe_op_in_unsafe_fn = "deny"`
newly in force across all 45 members — including the four crates that are
allowed `unsafe` at all (`sdk-ffi`, `cuda-miner`, `wgpu-miner`,
`ebpf-net/src/linux/`). Nothing had to be fixed for that.

4m 34s is a check, not a build. A real build of everything:

```
$ time cargo build --workspace --all-targets --message-format short
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4m 54s
real    4m55.214s
```

**CLAUDE.md's "52 minutes for a cold rebuild" is out by an order of
magnitude** at this commit. Part of that is the `line-tables-only` profile,
part is that the 52-minute figure predates it. The figure in CLAUDE.md is now
corrected.

That build also linked **every test binary at cargo's default parallelism (24
jobs on this machine) without `LNK1102: out of memory`**. CLAUDE.md had
recorded `CARGO_BUILD_JOBS=1` as mandatory for `cargo nextest run --workspace`,
with `exploit_replays` failing even at 4. Workspace-wide
`debug = "line-tables-only"` addressed the cause, and `.cargo/config.toml`
deliberately does not pin `jobs = 1`; this is the measurement behind that
decision.

### An incident worth recording: stale metadata-only units

The first two attempts at the test build failed, reproducibly and in seconds:

```
error: crate `regalloc2` required to be available in rlib format, but was not found in this form
error[E0463]: can't find crate for `custom_l1_node`
```

across `maya-faucet`, `maya-pool-service`, `l2-flash`, `maya-wallet-core`,
`maya-explorer`, `maya-cuda-miner` and others. It looked like the profile
change had broken linking.

It had not. After `cargo clean`, the first things run in this phase were
`cargo check --workspace --all-targets` and `cargo clippy --workspace
--all-targets`, both of which emit `.rmeta` and no `.rlib`. Cargo then
considered those units fresh when a later link step needed the `rlib`.
Cleaning the nine named dependencies fixed it:

```
$ cargo clean -p regalloc2 -p cranelift-frontend -p wasmtime-internal-cranelift \
      -p itertools -p cmov -p ark-poly -p ark-r1cs-std -p ahash -p tracing-subscriber
     Removed 373 files, 193.1MiB total
$ cargo build -p maya-explorer --bins
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 3m 27s
```

Recorded because the error message points at the profile and the cause is the
command order. On a CI runner the jobs are separate and start cold, so this
does not arise there; it arises for anyone who runs `check` or `clippy` before
`build` in a freshly cleaned tree.

## 5. Lints

`[workspace.lints]` switched on `clippy::pedantic = warn`,
`clippy::unwrap_used = warn` and `unsafe_op_in_unsafe_fn = deny` for all 45
members at once. The cost of that, measured:

```
$ cargo clippy --workspace --all-targets --message-format short
```

**1,700 file-anchored diagnostics.** Breakdown:

| Lint | Count |
|---|---|
| `cast_possible_truncation` / `cast_sign_loss` / `cast_precision_loss` (`casting …`) | 489 |
| `unwrap_used` | 457 |
| `doc_markdown` (missing backticks) | 308 |
| `casts from …` | 118 |
| `unwrap_err` | 47 |
| `needless_pass_by_value` | 32 |
| `redundant_closure` | 27 |
| `too_many_lines` | 18 |
| everything else | ~204 |

The decision — recorded in the workflow file where someone will meet it — was
**not** to deny pedantic. Denying it here would have meant either a
1,700-diagnostic cleanup inside a change whose subject is workspace layout, or
a gate no branch can pass. `warn` keeps it visible in every local run;
`PROGRESS.md` carries the debt.

What **is** denied in CI was calibrated against real runs rather than written
hopefully:

```
$ cargo clippy --workspace --lib --bins -- -D clippy::unwrap_used
    Finished. 0 warnings.
```

**Zero `unwrap()` in shipped code.** All 457 are in `#[cfg(test)]` modules and
test files, which `--lib --bins` does not compile. That is a gate that passes
today and means something tomorrow.

`expect_used` was measured at **73** in `--lib --bins` and is deliberately not
denied: an `expect("…")` that names the invariant it relies on is a different
object from a bare `unwrap()`. Tracked, not gated.

```
$ cargo clippy --workspace --all-targets -- -A clippy::pedantic -A clippy::unwrap_used -D warnings
    (exit 0)
```

Two real defects were found and fixed on the way:

- `clippy::needless_range_loop` in `neural-gas-trainer/src/network.rs:110`,
  which had been present at `223fe56` and which `00-inventory.md` originally
  mis-reported as "zero clippy warnings". Rewritten to iterate `hidden`
  directly; verified below that the trained weights did not move.
- `clippy::sort_unstable` / manual comparator in `xtask/src/disk.rs`, written
  during this phase.

## 6. Formatting

`cargo fmt --all --check` is a CI gate as of this phase, and the tree did not
pass it: **157 diffs across 55 files**. Formatted in a commit of its own.

This surfaced two latent problems, both now fixed:

**rustfmt reformatted a generated file.**
`fee-market/src/model/weights_v1.rs` is emitted by the neural-gas trainer,
whose `--check` mode compares the committed copy byte for byte against a fresh
run. Formatting it broke that comparison. `rustfmt.toml` now excludes the file.

**The drift check could never have passed on a fresh clone.** With
`core.autocrlf = true` and no `.gitattributes`, checkout rewrote LF to CRLF
while the generator writes LF:

```
$ cargo run -q -p maya-neural-gas-trainer -- --check
...weights_v1.rs differs from a fresh run: regenerate it
```

After adding `* text=auto eol=lf` and refreshing the working tree:

```
$ cargo run -q -p maya-neural-gas-trainer -- --check
float MSE 0.141388, quantization error 8 bps
linear: Metrics { blocks: 20000, mean_size_deviation: 0.14266636633872987, mean_fee_change: 0.017385393878901853, saturated_share: 0.0241, envelope_violations: 0 }
neural: Metrics { blocks: 20000, mean_size_deviation: 0.13937778244018556, mean_fee_change: 0.02073999535946687, saturated_share: 0.01845, envelope_violations: 0 }
...weights_v1.rs matches a fresh run
```

That also confirms the `needless_range_loop` rewrite in §5 changed nothing:
the compiled-in integer weights are identical.

## 7. The reality ledger

```
$ cargo xtask coverage
status  {"planned": 17, "verified": 43, "working": 30}
tier    {"core": 28, "extended": 41, "frontier": 21}
class   {"REAL": 43, "RESEARCH": 47}

features.toml: 90 entries, all claims backed.
```

```
$ cargo test -p xtask
running 4 tests
test disk::tests::human_rounds_to_the_unit_a_person_would_use ... ok
test disk::tests::an_empty_directory_measures_zero_and_a_missing_one_is_skipped ... ok
test coverage::tests::workspace_packages_includes_the_root_and_the_members ... ok
test coverage::tests::the_committed_ledger_parses_and_every_claim_is_backed ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

**90 entries, not the 164 the brief asked for.** No 162-item source exists in
this repository — `docs/trajectory.md` describes 160 prompts as twelve domain
*ranges*, not as named deliverables. The ledger holds what can be pointed at:
the 83 tagged rows of `docs/architecture-vision.md` plus 7 subsystems it has no
row for. Reasoning in [ADR-004](../docs/adr/ADR-004-reality-ledger.md).

## 8. Tests

The whole workspace, at cargo's default job count, with the new profile and
the new lint table in force:

```
$ time cargo nextest run --workspace --no-fail-fast --status-level fail
    Finished `test` profile [unoptimized + debuginfo] target(s) in 2.34s
────────────
 Nextest run ID ca8920df-d77b-4e50-8643-23fe29608243 with nextest profile: default
    Starting 2468 tests across 167 binaries (6 tests skipped)
────────────
     Summary [ 529.597s] 2468 tests run: 2468 passed (40 slow), 6 skipped

real    8m58.423s
```

**2,468 tests across 167 binaries. 2,468 passed, 0 failed, 6 skipped.**

Three things that measurement settles:

1. **`CARGO_BUILD_JOBS=1` is no longer required.** CLAUDE.md recorded it as
   mandatory, with `exploit_replays` hitting `LNK1102: out of memory` even at
   four jobs. This run linked 167 test binaries at 24 jobs and did not.
   `line-tables-only` removed the cause, which is why `.cargo/config.toml`
   deliberately does not pin `jobs = 1`.
2. **Nothing in this phase broke a test.** The `sha3` 0.10 → 0.11 bump, the
   `criterion` 0.5 → 0.7 bump, `blake3` → 1.8, 450 migrated dependency
   declarations, a workspace-wide lint table including
   `unsafe_op_in_unsafe_fn = deny`, six new profiles, a full `cargo fmt --all`
   across 55 files, and a `.gitattributes` that rewrote the working tree's
   line endings — 2,468 tests, all passing.
3. **The 6 skipped tests are skipped by the suite itself**, not by this run;
   they are the hardware-gated ones (`cuda`, `gpu`, `xdp`, `tpm`).

Not covered by this run, and not claimed: the nine non-member crates (six of
which cannot be built for the host at all — `00-inventory.md` §3), the fuzz
corpus, the Kani proofs, and the coverage floors. Those are `nightly.yml`.

## 9. What this phase did not deliver

- **Phase B, the directory move** (`crates/ bins/ apps/ hal/ sdks/ formal/
  infra/`). Deferred deliberately, with the reasoning in
  [ADR-001](../docs/adr/ADR-001-workspace-layout.md): it changes every `path =`
  dependency, every fingerprint, and paths in ten non-cargo files, and doing it
  in the same change as the workspace tables would make a failure
  unattributable.
- **Phase C, `sim/`.** The deterministic multi-node harness is not started.
  `tests/chaos_simulator.rs` and `tests/latency_sim_tests.rs` remain the
  closest thing, and neither is seed-replayable in the way the brief describes.
- **`attic/`.** Not created: three of the four duplications the brief asked to
  merge do not exist in this tree (`00-inventory.md` §6). Nothing was
  superseded, so nothing was moved.
- **CI observed green.** Every gate in `.github/workflows/ci.yml` was
  calibrated against a local run of the same command, but no workflow has run
  on a runner. Ubuntu-vs-Windows differences — notably the mold linker and the
  `--profile ci` paths — are unverified until the first push.
- **The 30 GiB ceiling after a full build** (§1).
