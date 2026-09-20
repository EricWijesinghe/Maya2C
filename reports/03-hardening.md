# 03 — Closing the gaps that would have cost something later

**Date:** 2026-09-20
**Branch:** `feat/complete-session-work`

`reports/01-foundation.md` and `reports/02-layout.md` each ended with a list of
things not done. This is the subset of those that would have gone on costing
something, finished. The rest were left alone on purpose, and §5 says which and
why.

---

## 1. The CI gates had never been executed

A gate that has never run is not a gate. Running each of them locally found two
that would have failed on the first push, and one that had been failing all
along.

### `cargo deny check` — was red, and had been

```
$ cargo deny check
advisories ok, bans ok, licenses FAILED, sources ok
```

11 crates under three licences the policy did not admit:

| Licence | Crates | Reached through |
|---|---|---|
| MPL-2.0 | the 9 `uniffi` crates | `maya-sdk-ffi` — uniffi *is* the product there |
| CDLA-Permissive-2.0 | `webpki-roots` | TLS roots, which are data not code |
| BSL-1.0 | `xxhash-rust` | transitive |

This is **not** new. `.github/workflows/security-audit.yml` has run
`cargo deny check advisories bans licenses sources` all along, so that job has
been failing; nobody had looked.

Fixed the way `deny.toml` says to fix it, which is a rule the file states
about itself: *"Every entry is permissive; there is no copyleft in the tree and
adding one should require editing this list. The one copyleft crate that exists
is admitted by name below, not by license."*

- `BSL-1.0` and `CDLA-Permissive-2.0` are permissive, so they go in `allow`.
- MPL-2.0 is file-level copyleft, so the nine uniffi crates are admitted **by
  name**, following the existing `dyn-eq` exception. Allowing the licence
  outright would admit the next MPL crate silently; allowing the crates means
  the next one fails and gets its own paragraph.

```
$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok
```

The duplicate `deny` job I had added to `ci.yml` is removed: `security-audit.yml`
owns that gate, and two jobs running it is a permanent cost for no extra
information.

### `cargo build -p maya-sdk-wasm --target wasm32-unknown-unknown` — had never worked

A step I wrote into `nightly.yml` without running it. It fails:

```
error: the wasm32/64-unknown-unknown are not supported by default; you may
need to enable the "wasm_js" crate feature
```

`docs/sdk.md` already recorded the artifact as unbuilt, attributing it to
`wasm-pack` not being installed. That was not the whole reason: **the crate had
never compiled for its own target.** A browser-bindings crate that has never
been compiled to wasm is exactly what `features.toml` exists to catch, and it
was marked `verified` on the strength of host-side parity tests.

`getrandom` refuses `wasm32-unknown-unknown` unless told where entropy comes
from — there is no OS to ask. It reaches this crate only transitively, at *two*
majors:

```
getrandom v0.4.3  <- maya-crypto-pq
getrandom v0.2.17 <- rand_core 0.6 <- fips204
```

A transitive dependency's feature cannot be enabled from outside, so both are
named in `sdks/sdk-wasm/Cargo.toml` under a `cfg(target_arch = "wasm32")`
table — for that reason and no other — with the matching
`--cfg getrandom_backend="wasm_js"` in `.cargo/config.toml`, scoped to that
target.

```
sdk-wasm host                                      OK
sdk-wasm wasm32                                    OK
workspace build                                    OK
```

`docs/sdk.md` now says what is and is not true: the crate compiles for wasm;
the *packaged* artifact and its size and signing figures are still unmeasured,
because `wasm-pack` is still not installed.

### The rest, verified rather than assumed

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --profile ci --workspace --lib --bins -- -D clippy::unwrap_used` | 0 |
| `cargo clippy --profile ci --workspace --all-targets -- -A pedantic -A unwrap_used -D warnings` | clean |
| `cargo nextest run --cargo-profile ci` | flag accepted, 23/23 on a probe crate |
| `bash scripts/doc_coverage.sh --check` | TOTAL 6582/6563 = 99%, floor 90% |
| `cargo xtask coverage` | 90 entries, all claims backed |
| frontier cross targets (`thumbv8m`, `riscv32imc`, `wasm32`) | all build |

`--profile ci` and `--cargo-profile ci` were both checked because a wrong flag
name fails a workflow on its first run and is invisible until then.

**Still not verified: any of this on a GitHub runner.** Every command above ran
on Windows. `mold`, the `apt` steps, the action versions and the runner's disk
budget are unexercised. That remains open and is the one item in this report
that a push would settle.

---

## 2. The 30 GiB ceiling was unreachable

`cargo xtask disk` reported 38.6 GiB against a 30 GiB ceiling, of which
**19.5 GiB sat in nested workspaces a root `cargo clean` never touches** and
the root profile never reached. A ceiling that cannot be met is a warning
people learn to ignore, and this volume has been filled to zero bytes twice.

The cause is structural: a crate with its own `[workspace]` table inherits
**none** of the root's profiles.

| Workspace | Had | Now |
|---|---|---|
| `apps/wallet-gui/src-tauri` (8.7 GiB) | no profiles at all | `debug = "line-tables-only"`, deps `opt-level = 3, debug = false` |
| `apps/dashboard` | no profiles at all | same |
| `apps/wallet-gui/ui` | `[profile.release]` only | same, plus its release profile |
| `fuzz` (10.2 GiB) | `[profile.release] debug = 1` | plus `[profile.release.package."*"] debug = false` |

`fuzz` keeps line tables for its own targets, because that is what a crash
report needs; a dependency's debug info is not — libFuzzer symbolises the frame
in the decoder under test, not in `wasmtime`.

The `opt-level = 3` half is not tuning either. Unoptimized post-quantum
cryptography makes a test run read as hung rather than slow
(`docs/invariants.md`, invariant 5), and a nested workspace hits that
independently — which is a trap this repository has already fallen into once.

---

## 3. 1,700 pedantic warnings with nothing watching them

`clippy::pedantic` is `warn` and not `deny`, for the reason in ADR-003.
Warned-but-untracked debt is debt that grows.

`scripts/lint_debt.sh` counts the diagnostics and compares them to
`lint-debt.txt`. The number may go down freely and may not go up.

```
$ bash scripts/lint_debt.sh --update
lint_debt: 1707 diagnostics (baseline 0)
lint_debt: baseline updated to 1707

$ bash scripts/lint_debt.sh --check
lint_debt: 1707 diagnostics (baseline 1707)
lint_debt: ok
```

1,707 rather than the 1,700 in `reports/01-foundation.md` §5: `sim` added
seven.

It runs in `nightly.yml`, not `ci.yml`. Each distinct set of clippy flags is a
distinct fingerprint, so this is a third full clippy pass, and debt *growth*
does not need to block a pull request — it needs to be impossible to miss.
When a change legitimately raises the number, `--update` records it and the
commit message says why, so an increase is a decision somebody made.

---

## 4. Result

```
$ cargo nextest run --workspace --no-fail-fast --status-level fail
    Starting 2515 tests across 169 binaries (6 tests skipped)
     Summary [ 487.540s] 2515 tests run: 2515 passed (40 slow), 6 skipped

$ cargo deny check
advisories ok, bans ok, licenses ok, sources ok

$ bash scripts/lint_debt.sh --check
lint_debt: 1707 diagnostics (baseline 1707)
lint_debt: ok

$ cargo xtask disk --check
target                                 26.4 GiB
apps/ledger-maya2c/target             611.3 MiB
hal/ebpf-net/programs/target          548.1 MiB
docs/site/node_modules                201.6 MiB
contracts/token-swap/target             1.4 MiB
target-contracts                       13.8 KiB
---------------------------------- ------------
total                                  27.7 GiB
ceiling 30.0 GiB
free on this volume: 366.6 GiB
--- exit 0 ---
```

**`cargo xtask disk --check` passes for the first time: 27.7 GiB against a
30 GiB ceiling.**

Two steps got it there, and the second matters as much as the first. Fixing
the nested profiles changes what future builds *cost*; it does nothing to the
19.5 GiB already on disk. Those bytes were built under the old profiles and
the change invalidated every one of them, so they were dead, not cached:

```
fuzz                                  Removed 9338 files, 10.2GiB total
apps/wallet-gui/src-tauri             Removed 6052 files, 8.7GiB total
apps/wallet-gui/ui                    Removed 1436 files, 255.4MiB total
```

The real cost is that the wallet GUI and the fuzz corpus rebuild next time
they are used. That was unavoidable the moment the profiles changed.

The full suite was re-run after every change in this report, not just at the
end: 2,515 tests across 169 binaries, all passing, the same count as before
the hardening began.

---

## 5. Deliberately still open

These were judged not worth closing, and the judgement is the point — each one
is cheap to do and wrong to do.

- **`features.toml` holds 90 entries, not the 164 the brief asked for.** There
  is no 162-item source in this repository. Padding it would make the gate
  report on features nobody can point at, which inverts its purpose (ADR-004).
- **`chacha20poly1305` 0.10/0.11 and `rand`/`rand_core` remain split.**
  Unifying the first rewrites AEAD call sites in crypto code; the second is a
  major-version split with a reason, which is not drift (ADR-005).
- **`ed25519-dalek` 2.2 against libp2p's 3.0.** Must be closed before
  `THREAT_INTEL_ACTIVATION_HEIGHT` ever moves, and that is where ADR-005 puts
  it. The subsystem is dark; closing it now would be a crypto change inside a
  build-system phase.
- **73 `expect_used` in shipped code.** An `expect("…")` that names the
  invariant it relies on is a different object from a bare `unwrap()`. Denying
  both would push people toward `unwrap_or_default`, which is worse.
- **`chaos_simulator.rs` and `latency_sim_tests.rs` are not ported onto
  `sim/`.** They work. Porting them changes test behaviour and belongs in a
  commit whose subject is that.
- **CI on a real runner.** Cannot be settled without pushing.
