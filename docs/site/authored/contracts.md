---
title: Compiling a contract
description: Building a WASM contract for the Maya VM, and the limits it must fit inside.
---

Contracts are `wasm32-unknown-unknown` cdylibs. `contracts/token-swap/` in the
repository is a working example and the best starting point.

## The limits, before you write anything

| Limit | Value | Source |
|---|---|---|
| Module size | 512 KiB | `MAX_MODULE_BYTES` |
| Linear memory | 16 MiB (256 pages) | `MAX_MEMORY_PAGES` |
| Guest stack | 512 KiB | `MAX_STACK_BYTES` |
| Gas | per call, named by the caller | wasmtime fuel |

These are **fixed numbers, not fractions of host memory**. A limit that varied
with the machine would let the same call succeed on one validator and trap on
another, which is a state divergence rather than a performance difference.

`MAX_MODULE_BYTES` and `MAX_MEMORY_PAGES` are governable — they appear in the
parameter table as `VmMaxModuleBytes` and `VmMaxMemoryPages`. The values above
are the defaults; a live chain reads them from state.

## Gas is a halting bound, not a price

Gas maps one-to-one onto wasmtime fuel. It exists so a contract cannot run
forever, not to charge for computation — there is no fee market on this chain
today, and nothing bills the gas a call consumes.

Read that as: your contract must **terminate**, and the ceiling is generous.
Do not micro-optimise instruction counts.

## Its own workspace, deliberately

```toml
# contracts/token-swap/Cargo.toml
[workspace]        # empty table: excluded from the host workspace

[lib]
crate-type = ["cdylib"]
```

The empty `[workspace]` table is what keeps a contract out of the node's
workspace. Pulling it in would drag the node's dependency graph — RocksDB,
arkworks, libp2p — into a target that cannot compile any of it.

## Building

```bash
rustup target add wasm32-unknown-unknown

cd contracts/token-swap
cargo build --release --target wasm32-unknown-unknown

ls -l target/wasm32-unknown-unknown/release/token_swap.wasm
```

Check the size against the 512 KiB ceiling. If you are close, the usual causes
are `panic = "abort"` not being set, formatting machinery pulled in by
`format!`, or a dependency that assumed `std`.

```toml
[profile.release]
opt-level = "z"
lto = true
panic = "abort"
codegen-units = 1
strip = true
```

## Determinism

The engine is configured deterministically: no floating-point non-determinism,
no threads, no ambient time, no randomness. Anything a contract cannot compute
from its inputs and chain state, it cannot compute at all.

If you need randomness, read the beacon — it is a VRF output folded per block,
and a contract sees the **previous** block's value. That ordering is
deliberate: a beacon folded before execution would be a value the block's own
transactions could have been written against.

## Deploying

A deployment is a transaction carrying the module bytes. It is rejected if the
module exceeds the size limit, fails validation, or uses a disabled WASM
proposal — checked before anything is stored, so a bad module costs a rejected
transaction rather than chain space.

## What a contract cannot do

- **No network, no filesystem, no clock.** The host functions are the whole
  interface.
- **No native code.** Nothing fetches code from chain state and runs it —
  including governance, which can move a number or flip between two
  implementations the binary already ships, and nothing else.
