# The VM Module Cache

Compiling a contract once instead of on every call.

- `crates/vm/src/cache.rs` — the cache.
- `crates/vm/src/config.rs` — `config_digest`, which is half the key.
- `crates/vm/tests/cache_tests.rs`, `crates/vm/benches/module_cache.rs`.

---

## There was never an interpreter

The brief that produced this asked for an adaptive JIT and a 10× improvement of
"JIT over interpreted". `crates/vm/src/config.rs` sets `Strategy::Cranelift`, so
wasmtime has always compiled every module to native x86-64 or aarch64 before
running it. There is no interpreted mode in this tree, and adding one so the
benchmark had something to beat would be a number manufactured to be met.

What was actually missing: `Vm::call` handed the same bytes to `Module::new` on
**every call**, and Cranelift optimised them from scratch each time. The engine's
own caches do not cover that.

## Measured, not targeted

| contract | recompiled | cached | ratio |
|---|---:|---:|---:|
| trivial (returns immediately) | 875.99 µs | 38.05 µs | **23.0×** |
| busy (20k-iteration loop) | 764.93 µs | 54.59 µs | **14.0×** |

The trivial figure is the flattering one — almost all of that time was
compilation, so it measures the compiler more than the contract. **14× is the
honest number**: a contract doing real work, where compilation is a smaller share
of the call.

Both clear the brief's 10×. That is a coincidence of where the numbers landed,
not a target the benchmark was shaped around; `cargo bench -p maya-vm` reports
whatever the machine gives.

## Why a cache cannot change gas

Gas is wasmtime fuel (invariant 21), and fuel is instrumentation the compiler
injects per Wasm operator. The same bytes under the same configuration produce
the same instrumentation, so a cached module charges exactly what a freshly
compiled one charges.

`a_cached_call_burns_identical_fuel` asserts it rather than assuming it, and
`a_cleared_cache_still_charges_the_same` asserts the other direction — a node
that restarts mid-chain has a cold cache and must agree with one that did not.
"Gas is unchanged" is the entire safety argument for putting a cache on a
consensus path, so it is a test and not a comment.

## The configuration is half the key

Not just the bytecode. A cache keyed on bytes alone would, after a change to
`deterministic_config` — NaN canonicalisation, a proposal flag, the stack cap —
keep serving modules compiled under the **old** settings until the process
restarted. Two nodes would then run one contract under two compilers, which is
the one way a cache can fork a chain.

So the key is `BLAKE3(config_digest || len || bytecode)`, and the digest covers
the wasmtime version too. `Cargo.toml` already pins wasmtime exactly because
*"different wasmtime versions can compute different gas for the same call"* — a
cache that survived a version bump would be that hazard with a longer fuse.

`config_digest` is a hand-written list, because wasmtime exposes no stable
fingerprint of a `Config`. That makes it something somebody must extend when
they add a setting.

## Bounded, and evicting the coldest

256 modules. An unbounded cache is a node that dies from the number of distinct
contracts it has seen — a number an attacker chooses, by deploying junk. The
entry untouched longest goes first, so a contract called once by an attacker
does not evict one called every block.

"Least recently used" is measured by a monotonic counter, not a clock. This runs
inside block execution, where nothing may read wall time.

## Status

**SHIPPED**, and it changes no consensus rule: same fuel, same traps, same host
surface, same output. The only difference is that a node compiles a contract
once instead of every time it is called.
