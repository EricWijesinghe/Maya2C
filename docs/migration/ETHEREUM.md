# From Ethereum

## The path the brief assumed does not exist

The brief says "Solidity via the EVM layer, then optional port to WASM".
There is no EVM layer. ADR-023 keeps it out of v1, and any future one will be
labelled *classical security*, because secp256k1 wallets do not survive a
quantum attacker. The only path is to port to Rust compiled to WASM.

## Concept map

| Ethereum | Maya2C | Notes |
|---|---|---|
| EOA (secp256k1) | Account keyed by an ML-DSA-65, SLH-DSA or hybrid suite | Addresses are hashes of PQ keys. A transfer is ~13 KB on the wire (`reports/13-pq-weight.md`) |
| Smart wallet (ERC-4337) | Native smart account (`crates/smart-account`) | Session keys, limits, guardians and delays are built in. See `reference-apps::payments` and `::vault` |
| `msg.sender` | **none yet** | ADR-026. Every `require(msg.sender == …)` is the line you cannot port |
| `tx.origin` | none | No contract-to-contract calls, so the distinction does not arise |
| `block.number` | `block_height()` host call | |
| `block.prevrandao` | `block_randomness()` | The previous block's VRF beacon |
| Storage slots | `storage_read` / `storage_write` on byte keys | The whole contract storage is loaded per call, so keep it small |
| Events / logs | `emit_event(topic, data)` | |
| Chainlink feed | `oracle_read`, `oracle_feed_age` | The native oracle (ADR-024) |
| ERC-20 | **no token standard** | `rwa` has regulated tokens. There is no plain fungible-token contract standard |
| ERC-721 | `contracts/nft-game` (blocked by ADR-026) | |
| Uniswap v2 pool | `contracts/token-swap`, or the native `dex` module | The native module settles each block's swaps at one price (ADR-025) |
| Governor + Timelock | native `governance` plus `treasury` | See `reference-apps::dao` |
| `ecrecover` / signature checks in contracts | none | No verify host function |
| `delegatecall` proxy upgrades | none | Code is immutable. Governance cannot inject code |
| Hardhat / Foundry | `cargo build --target wasm32-unknown-unknown`, then tests against `maya_vm::runtime::Vm` | See `crates/reference-apps/tests/nft_game.rs` for the harness. There is no fork mode or debugger (`reports/24-devx.md`) |

## Porting checklist

1. Remove every `msg.sender` check, and write down what it protected. Those
   lines wait for ADR-026.
2. Replace `mapping` with byte-keyed storage. Prefix each key by kind, as
   `nft-game` does with `o`, `l` and `h`.
3. Replace `uint256` with `u64`/`u128` and checked arithmetic. Overflow traps,
   because the release profile keeps `overflow-checks = true`.
4. Replace external calls with a native-module design or a single contract.
   Nothing can call another contract.
5. Keep the module small. The limit is 512 KiB, and token-swap and nft-game
   are 2–3 KiB.
6. Test in the VM (`Vm::execute` with `MemoryState`) before you touch a node.
