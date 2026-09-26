# From Solana (Anchor)

## Concept map

| Solana / Anchor | Maya2C | Notes |
|---|---|---|
| Program (BPF) | WASM contract | Built with `cargo build --target wasm32-unknown-unknown`, `no_std`, no allocator |
| Accounts passed in the instruction | the contract's own byte-keyed storage | A contract reads only its own storage, plus the balances of the caller and of itself |
| `Signer<'info>` constraint | **none yet** | ADR-026. The runtime knows the signer and does not tell the contract |
| PDA (program-derived address) | storage key prefix | There is no address to derive. State is keyed inside the contract |
| CPI (`invoke`, `invoke_signed`) | **none** | Contracts cannot call contracts. Compose through native modules |
| SPL Token | **no token standard** | See ETHEREUM.md for the same gap |
| Clock sysvar | `block_height()` | There is no wall-clock time, by design |
| Priority fees / local fee markets | `lanes` (RESEARCH) | Per-app fees and reserved lanes, simulated. They are not the node's fee rule |
| Compute units | fuel (gas) | This bounds execution. Nothing is billed |
| Anchor IDL | the ABI table in the contract's module doc | There is no IDL generator. Byte 0 selects the method |

## Porting checklist

1. Collapse the accounts model into contract storage. Each account your
   instruction took becomes a key prefix.
2. Remove the `Signer` checks, and write down what they protected. They wait
   for ADR-026.
3. Replace each CPI with either one contract or a native module.
4. Make the Borsh encoding explicit: the ABI is raw little-endian bytes.
