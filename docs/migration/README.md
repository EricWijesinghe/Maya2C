# Migration kits

For teams porting from Ethereum, Solana and Move chains (Master Prompt 30 §2).
Each kit maps concepts, lists what does not port, and points to the
reference code that shows the Maya2C equivalent.

| Coming from | Kit | The first thing that will not port |
|---|---|---|
| Ethereum / Solidity | [ETHEREUM.md](ETHEREUM.md) | There is no EVM layer (ADR-023), so you port to Rust→WASM. `msg.sender` has no equivalent yet (ADR-026) |
| Solana / Anchor | [SOLANA.md](SOLANA.md) | The `Signer` constraint has no equivalent yet (ADR-026). There are no cross-program calls |
| Aptos / Sui / Move | [MOVE.md](MOVE.md) | Resources are a runtime library (`contract-safety`), not bytecode-verified types |

## Measured port times

The brief asks for the time to port each reference app. Only one of the five
is a contract port. The other four compose native modules (smart accounts,
the AMM, governance, the treasury), so they have no source chain to port from.

| App | From | Port | Wall clock | Who | Result |
|---|---|---|---|---|---|
| NFT game (`contracts/nft-game`) | ERC-721 subset plus levels, Solidity shape | Rust `no_std` → WASM, 2,161 B module | **81 s** from first line to passing VM tests | an AI agent that had just read `contracts/token-swap` | Logic ported; **authorisation cannot be** (ADR-026) |

How to read the 81 s:

- It is one data point, not an estimate for a team.
- The agent already knew the VM's ABI from token-swap, and it measured itself.
- A human port time needs a human porting, and none has.
- The number mostly shows what the ABI costs once it is known: static
  buffers, a hand-rolled argument decoder, and no SDK macros.

## What every kit says first

1. **Amounts are base units.** No symbol or decimals are ratified yet
   (`chain/maya2c-testnet.json`).
2. **Contracts are metered, not billed.** Gas bounds execution, and the fee
   market is inactive.
3. **There are no contract-to-contract calls.** `env.call` is deliberately
   absent, which rules out re-entrancy and composability alike.
4. **A contract cannot learn its caller** until ADR-026 is decided.
