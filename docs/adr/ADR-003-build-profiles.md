# ADR-003: Build profiles and the disk ceiling

**Status:** Accepted
**Date:** 2026-09-20

## Context

At commit `223fe56` this workspace had no `[profile.dev]` and no
`[profile.release]`. It had thirty `[profile.dev.package.*]` overrides, each
one a hand-placed `opt-level = 3` on a crate whose unoptimized form made
`cargo test` read as hung rather than slow — that is critical invariant 5, and
those overrides are load-bearing.

What it did not have was any statement about the profiles themselves. Release
builds used cargo's defaults: no LTO, sixteen codegen units, symbols kept.

Measured with `cargo xtask disk` at that commit:

```
target                                316.0 GiB
fuzz/target                            10.2 GiB
wallet-gui/src-tauri/target             8.7 GiB
app-maya2c/target                     611.3 MiB
ebpf-net/programs/target              548.1 MiB
wallet-gui/ui/target                  255.4 MiB
docs/site/node_modules                201.6 MiB
contracts/token-swap/target             1.4 MiB
target-contracts                       13.8 KiB
----------------------------------------------
total                                 336.5 GiB
free on this volume                    85.6 GiB
```

This volume has been filled to zero bytes twice. When it happens cargo reports
`os error 112` and rustc exits `0xc0000409`, neither of which names the
problem. The brief set a 30 GiB ceiling.

Debug information is the bulk of it. The tree already knew this: the override
`[profile.dev.package.custom-l1-node] debug = "line-tables-only"` exists
because MSVC `link.exe` refuses to produce the node binary at all past a PDB
module cap (`LNK1140`), and `cargo nextest run --workspace` could not even
build without it. The same override was scoped to one package specifically so
dependencies would keep full debug info.

## Decision

```toml
[profile.dev]
debug = "line-tables-only"
incremental = true

[profile.dev.package."*"]
opt-level = 3
debug = false

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 16
debug = false
strip = "debuginfo"

[profile.dist]
inherits = "release"
lto = "fat"
codegen-units = 1
strip = "symbols"

[profile.ci]
inherits = "dev"
debug = false
incremental = false

[profile.fuzz]
inherits = "release"
debug = "line-tables-only"
debug-assertions = true
overflow-checks = true
strip = "none"
```

`cargo xtask disk --check` exits non-zero above 30 GiB and is a CI step.

Three of the brief's settings were changed, each for a reason that would
otherwise be rediscovered:

**`panic = "abort"` is not set.** Cargo applies `panic` to a whole profile,
not to binaries. Setting it breaks `cargo test` — the harness needs to unwind
to report a failure — and conflicts with the `cdylib` targets in `sdk-ffi` and
`sdk-wasm`. A binaries-only abort needs its own profile and its own full build
of every dependency, which costs a second `target/` to save some binary size.

**`lto = "fat"` moved to `dist`.** Fat LTO over RocksDB, wasmtime, arkworks,
halo2 and libp2p in one unit is an hours-long link. `release` uses thin LTO,
which gets most of it; `dist` is for tagged builds that can afford the wall
clock.

**`split-debuginfo` is not set.** Its accepted values and their meanings
differ between MSVC, ELF and macOS; a cargo profile cannot be made conditional
on the host; and `line-tables-only` has already moved most of what it would
move.

## Alternatives considered

**Keep `debug = "line-tables-only"` scoped to `custom-l1-node`.** That was the
status quo and it left the comment's stated trade-off intact (dependencies
keep full debug info, so stepping into them works). Rejected: the trade was
made when the problem was one binary failing to link. The problem now is
316 GiB, and workspace-wide line tables are the single largest lever on it.
File and line in a backtrace survive; inspecting a *dependency's* local
variables in a debugger does not.

**`opt-level = 2` for dependencies instead of 3.** Would build faster. Rejected
because the thirty existing overrides already chose 3 for exactly these crates
and invariant 5 records what happens without it; lowering it now would be
changing a measured decision on a guess.

**Delete the thirty per-package overrides as redundant.** Partly true and
dangerous. `[profile.dev.package."*"]` matches dependencies, **not workspace
members**, so the overrides on `maya-crypto-pq`, `maya-cuda-miner`,
`maya-zkml`, `maya-zkml-prover`, `maya-htlc-lattice` and
`maya-stateless-core` are still the only thing optimizing those crates — and
invariant 2 depends on `maya-crypto-pq` specifically being the optimized one.
They stay.

## Consequences

Measured after the change, on the machine above
(`reports/01-foundation.md` §1, §4, §8):

| | Before | After |
|---|---|---|
| root `target/`, everything built and tested | 316.0 GiB | **29.6 GiB** |
| all nine artifact directories | 336.5 GiB | 50.2 GiB |
| `cargo build --workspace --all-targets`, cold | ~52 min (recorded) | **4m 54s** |
| `cargo nextest run --workspace` | needed `CARGO_BUILD_JOBS=1` | 2,468 tests pass at 24 jobs |

- **Every fingerprint in `target/` was invalidated**, which is why this change
  was made together with the lint policy and immediately before a
  `cargo clean`: doing it later would have meant a second full rebuild, on a
  volume that did not have room for two copies.
- Cold build time did **not** go up, despite every dependency now compiling at
  `opt-level = 3` where before only the thirty named ones did. The saving from
  not writing full debug info is larger than the cost of optimizing.
- The `LNK1102: out of memory` failure that forced `CARGO_BUILD_JOBS=1` is
  gone, because its cause was debug-info volume per link. `.cargo/config.toml`
  therefore does not pin `jobs = 1`.
- The 30 GiB ceiling is met by the root `target/` and **not** by the sum. The
  20.6 GiB that survives a root `cargo clean` belongs to `fuzz/`,
  `wallet-gui/src-tauri/` and `app-maya2c/` — separate workspaces with their
  own profiles, which this ADR does not reach.
- Debugging a dependency's locals now needs a one-off build with
  `RUSTFLAGS=-Cdebuginfo=2` or a temporary package override.
- `sccache` stays off. It does not cache incremental builds, so it and
  `incremental = true` defeat each other; this tree keeps incremental.

## Revisit when

`cargo xtask disk` reports the workspace comfortably under the ceiling with
room to spare — at which point `debug = 2` for workspace members becomes
affordable again — or when a measurement shows the cold-build cost of
`opt-level = 3` on all dependencies exceeds what it saves.
