# From Move (Aptos, Sui)

Move's safety model is the closest to what `crates/contract-safety` offers,
and also where the difference matters most.

## Concept map

| Move | Maya2C | Notes |
|---|---|---|
| Resources (linear types, bytecode-verified) | `contract-safety` move-only resources | These are a **runtime library**, not verifier-enforced types. The VM still runs plain WASM. `contract-safety` is not wired into the VM (RESEARCH) |
| Abilities (`key`, `store`, `copy`, `drop`) | none in the VM | Emulate them with the library's resource checks |
| `signer` argument | **none yet** | ADR-026 |
| Capabilities pattern | `contract-safety` capability-gated access | In the library, and not enforced on chain |
| Sui objects and ownership | contract storage keyed by object id | There is no parallel scheduling from object ownership. `parallel-exec` (RESEARCH) uses declared access lists |
| Module upgrade policies | none | Code is immutable |
| Events | `emit_event` | |
| Move Prover specs | Kani proofs on native modules (`ledger-math`, `dex`, `governance`, `fee-market`) | This covers native modules only, not contracts |

## Porting checklist

1. List your resources and their abilities. What the Move verifier guaranteed,
   you now guarantee in code and tests.
2. Remove the `signer` checks and wait for ADR-026. Move developers will find
   this the strangest gap.
3. Replace object-parallelism assumptions with sequential execution. That is
   today's model.
