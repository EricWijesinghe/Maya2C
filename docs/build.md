# Building and testing Maya2C

Everything in this file is measured on the development machine and dated.
`CLAUDE.md` carries the short version and links here.

Moved out of `CLAUDE.md` on 2026-09-20 to keep that file inside the 300-line
budget the foundation brief set for it.

Never run unfiltered cargo output — use `qb` / `qt` / `ql`, or pipe through
`condense`.

```powershell
cargo xtask disk                                # artifact size vs the 30 GiB ceiling
cargo xtask coverage                            # the reality ledger, and whether claims are backed
qb                                              # cargo build --workspace
qt                                              # cargo nextest run --workspace
ql                                              # cargo clippy --all-targets
cargo llvm-cov --workspace --summary-only       # 80% floor
bash scripts/doc_coverage.sh --check            # doc coverage (90% floor) + no broken doc links
cargo deny check                                # deny.toml is committed
cargo audit
cargo fuzz list                                 # fuzz/ targets (nightly; decoders + SV2 frames)
cargo machete                                   # unused-dependency *candidates* — heuristic, verify each
```

`cargo machete` flags `maya-sdk-ffi -> maya-crypto-pq`, a dependency that
exists for invariant 2 rather than for any `use`. Check every hit against the
invariants before removing it.

**Disk is the standing hazard.** This volume has been filled to exactly zero
bytes twice, and a full disk reports as `os error 112` or rustc `0xc0000409` —
never as "out of space". Run `cargo xtask disk` *before* starting a long build,
not after it fails. Measured 2026-09-20: 336.5 GiB across all artifact
directories before `cargo clean`, 20.6 GiB immediately after, and 50.2 GiB
once everything had been rebuilt and tested. The root `target/` is 29.6 GiB
of that, down from 316.0 GiB. `fuzz/target` (10.2 GiB) and
`apps/wallet-gui/src-tauri/target` (8.7 GiB) survive a root clean; they are
separate workspaces with their own profiles, and they are why the *sum* is
still over the 30 GiB ceiling while the root is under it.

**Profiles are load-bearing, not tuning** — [ADR-003](docs/adr/ADR-003-build-profiles.md).
`[profile.dev] debug = "line-tables-only"` and
`[profile.dev.package."*"] opt-level = 3, debug = false` are what keep the tree
inside the ceiling and the node binary under MSVC `link.exe`'s PDB module cap
(`LNK1140`, past which `cargo nextest run --workspace` cannot even build). The
per-package overrides that name *workspace members* are not redundant with the
`"*"` override — `"*"` does not match members, and invariant 2 depends on
`maya-crypto-pq` being the optimized crate. Do not "clean them up".

Measured on this machine, 2026-09-20, from a clean tree:

| Command | Cold |
|---|---|
| `cargo clean` | 1m 28s, 188,085 files, 317.4 GiB |
| `cargo check --workspace --all-targets` | 4m 34s, 0 errors, 0 warnings |
| `cargo clippy --workspace --all-targets` | 1,700 diagnostics, all `pedantic` or `unwrap_used` |
| `cargo build --workspace --all-targets` | 4m 54s |
| `cargo nextest run --workspace` | 2,515 tests across 169 binaries, **2,515 passed**, 6 skipped, 480.4s (at `jobs = 4`) |
| root `target/` afterwards | **29.6 GiB**, from 316.0 GiB |

`cargo clippy --workspace --lib --bins -- -D clippy::unwrap_used` reports
**zero**: all 457 `unwrap()` warnings are in `#[cfg(test)]` code. That is the
CI gate.

**A cold rebuild is about five minutes, not 52.** The 52-minute figure in this
file predated the profile change and is corrected.

**Build parallelism is still bounded, and this file was briefly wrong about
it.** One run of `cargo nextest run --workspace` at cargo's default 24 jobs
succeeded, and that was written up here as "`CARGO_BUILD_JOBS=1` is no longer
needed". Every later run at 24 jobs failed. The failure is not `LNK1102` any
more — with `line-tables-only` the linker is fine — it is rustc:

```
error[E0786]: found invalid metadata files for crate `custom_l1_node`
  = note: failed to mmap file 'target\debug\deps\libcustom_l1_node-*.rlib':
          The paging file is too small for this operation to complete. (os error 1455)
```

`os error 1455` is Windows commit-charge exhaustion. Measured on this machine:
31.4 GB RAM, a 22 GB pagefile, **commit limit 53.6 GB with 12.9 GB free**. Two
dozen `rustc` processes compiling wasmtime, arkworks and halo2 at
`opt-level = 3`, each mmapping a large rlib, go past it. What it looks like
from the outside is `can't find crate` and `required to be available in rlib
format` cascading across the workspace — which reads as a cargo, profile or
linker bug and is none of them.

The bound lives in `.cargo/config.toml` and `CARGO_BUILD_JOBS` overrides it.

**Run one cargo scope at a time.** Interleaving `cargo build -p <crate>`, or
`cargo xtask …` (which is `cargo run -p xtask`), with a
`cargo build --workspace` wedges the target directory:

```
error: crate `wasmtime_internal_cranelift` required to be available in rlib format, but was not found in this form
error[E0463]: can't find crate for `custom_l1_node`
```

Under resolver 2/3 the unified feature set for a shared dependency differs
between a `-p` scope and a `--workspace` scope, so the two builds produce
different units into the same directory and each invalidates what the other
needs. It reads as a profile or a linker bug and is neither.
`CARGO_BUILD_PIPELINING=false` does not help. The remedy is to let one scope
finish before starting another; `cargo clean` if it is already wedged. See
`reports/02-layout.md` §1.
