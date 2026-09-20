# tests/

Empty of tests, and it cannot be otherwise.

The foundation brief lists `tests/` at the repository root. That worked while
`custom-l1-node` *was* the root package - a root `tests/` directory belonged
to it. Since the phase B move (`docs/adr/ADR-001-workspace-layout.md`) the
root manifest is a **virtual workspace**, and a virtual manifest owns no
targets: cargo would not compile anything placed here.

The integration tests moved with the crate they test:

| Where | What |
|---|---|
| `crates/node/tests/` | 47 suites - consensus, state, dex, sealed mempool, exploit replays, chaos |
| `crates/*/tests/`, `hal/*/tests/` | each crate's own |
| `sim/tests/` | the determinism harness |
| `fuzz/fuzz_targets/` | 16 decoders, its own workspace |

`cargo nextest run --workspace` runs all of them: 2,515 tests across 169
binaries.
