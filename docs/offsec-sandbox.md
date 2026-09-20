# offsec-sandbox: coverage-guided differential fuzzing

**Status: RESEARCH, off by default.** A tooling crate, not a node dependency.
Nothing in `crates/node/src/` reaches it; the node's dependency graph, `cargo deny`, and Kani
never see it. It is a separate workspace root, exactly as `fuzz/` is, and for the
same reason.

Code: `offsec-sandbox/` (its own `[workspace]` and lockfile). The in-tree gate
is `crates/node/tests/fuzz_harness.rs`, which shares no dependency with the crate.

## The brief, and what it maps to on this chain

| Asked | Built | Why the difference |
|---|---|---|
| Integrate LibAFL and Triton/Z3 **into the development and testing execution path** | A separate workspace crate that only *tooling* runs. LibAFL is an optional engine; Z3 is an off-by-default feature over the arithmetic crates; Triton is dropped | `fuzz/Cargo.toml` is already its own workspace so the sanitizer/libFuzzer runtime "the rest of the tree must never link" stays out. LibAFL pulls a large graph, Z3 a C++ library — putting either on the node's path breaks invariants 1 and 6 and the Kani story. Triton is a C++ x86/ARM DSE framework: there is no native binary to concolically run here, and no Rust binding |
| Mutate tx bytes, WASM bytecode, P2P handshakes for **memory corruption, state race conditions, DoS** | Structure-aware mutators for those three surfaces, hunting **panics, unbounded allocation, and non-determinism** | Safe Rust has no memory corruption outside the `unsafe` islands (which ASan covers). The apply path is single-threaded by construction, so there is no data race to fuzz — the real concurrency property is **determinism** (invariant 24, execution directive 2), a differential property, not a race |
| An **exploit verification pipeline that generates patch diffs** | Crash **triage**: minimize, dedup by panic site, classify, and emit a failing regression-test stub. A human writes the fix | A fuzzer writing a patch to consensus code is invariant 13 in spirit — machine-authored code changing chain behaviour — and patch synthesis from a crash is unsolved. Triage is the part that is real |
| `crates/node/tests/fuzz_harness.rs` running **1,000,000 payloads to verify 100% crash resistance** | A deterministic seeded-mutation replay through the decoders and the apply path, default 50,000 iterations, `MAYA_FUZZ_ITERS=1000000` for a soak, asserting **no panic and no non-determinism** | A `crates/node/tests/` test runs on the node's stable toolchain in the main workspace — it cannot link LibAFL/ASan. And fuzzing proves no crash was *found*, never that none *exists*: "100%" stated as fact is the guessed-vector failure this repo keeps catching. The open-ended search stays in the LibAFL workspace |

## Why a separate workspace, restated

Three crates in this tree — `ledger-math`, `dex`, `governance` — have deliberately
tiny, C-free dependency graphs so Kani can compile them (invariants 1, 6). The
node links RocksDB, wasmtime, arkworks, halo2. LibAFL adds a large graph and Z3
adds a C++ library. If any of that were reachable from a node crate:

- `cargo machete`/`cargo deny`/`cargo tree` would carry it forever;
- a `[features]` unification could switch it on in a crate that never asked;
- Kani's goto-binary compile of `ledger-math` could stop working.

So `offsec-sandbox` sits outside the workspace and depends on the node crate
one-directionally, the way `fuzz/` does. The shared mutation logic that the
in-tree harness also needs is small enough to live in the harness itself rather
than be pulled from this crate — the same trade the tree makes elsewhere (a
duplicated hybrid derivation pinned by a parity test).

## What each surface's mutator produces

Byte-level mutation of a wire format spends almost all its budget being rejected
by `from_bytes` at offset 0. The value is in *structure-aware* mutation that
gets past the decoder and reaches logic:

- **Transactions** (`mutate::tx`): a decoded `Transaction` mutated field-wise —
  amounts to `u64` boundaries, nonce, output count, payload kind — then
  re-encoded, sometimes re-signed and sometimes left with a stale signature, so
  both the decode path and the signature-check path are exercised.
- **WASM modules** (`mutate::wasm`): `wasm-smith` generates modules within the
  VM's import set, and `wasm-mutate` perturbs a valid seed. The target is the
  deploy validator and the fuel meter, not arbitrary bytes — those the
  `payload_decode` fuzz target already covers.
- **Handshakes** (`mutate::handshake`): the PQ transport and relay frame
  structures, mutated at field boundaries (lengths, version bytes, key ids).

## Determinism is the real concurrency property

The apply path is single-threaded: `Chain::insert_block` is `&mut self` and the
node driver owns it on one task. There is no data race to fuzz. What can go
wrong is that the *same block* executes to a *different state root* on a second
machine or a second run — which execution directive 2 and invariant 24 forbid.
The differential harness applies each mutated block to two independent
`StateDB`s and asserts the roots agree, and that a block the chain accepts,
re-applied, produces the same root.

## Triage, not patching

When an input panics, diverges, or is refused where it should not be, the
pipeline:

1. **minimizes** it (shrink toward the smallest input with the same outcome);
2. **classifies** it — decoder panic / apply-path `Err` that should have been a
   clean refusal / determinism divergence / out-of-memory;
3. **dedups** by panic site or divergence signature;
4. emits a **regression-test stub** in the shape of `crates/node/tests/exploit_replays.rs`,
   which asserts the *reason* a fixed input is now handled, not merely that it
   no longer crashes.

It does not write a patch. A patch to consensus code is a human decision, and
this crate has no authority to make chain behaviour change on its own — the same
line invariant 13 draws for governance.

## What this is not

- **Not a proof.** A clean fuzz run is evidence, not a guarantee. The doc and
  the harness say "no crash across N inputs," never "crash-proof."
- **Not a second corpus.** The LibAFL runners read and write the same corpora as
  the `fuzz/` libFuzzer targets, so a finding on one improves the other.
- **Not on the consensus path.** Nothing here runs in a node. The one in-tree
  artifact is a test.

## Running

```text
# the fast gate (main workspace, stable, no extra deps)
cargo test --test fuzz_harness
MAYA_FUZZ_ITERS=1000000 cargo test --test fuzz_harness --release   # soak

# the coverage-guided search (separate workspace, nightly)
cd offsec-sandbox
cargo +nightly-2026-07-15 run --features libafl --bin fuzz -- tx      --time 900
cargo +nightly-2026-07-15 run --features libafl --bin fuzz -- wasm    --time 900
cargo +nightly-2026-07-15 run --features libafl --bin fuzz -- handshake --time 900

# boundary-input generation with a solver (optional, arithmetic crates only)
cargo run --features z3 --bin solve -- ledger-math
```
