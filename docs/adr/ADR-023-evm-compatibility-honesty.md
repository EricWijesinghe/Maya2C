# ADR-023: EVM compatibility is not in v1, and when it comes it is labelled classical

**Status:** Accepted
**Date:** 2026-09-26

## Context

Master Prompt 17 §3 asks for an `eth_*` JSON-RPC layer over revm, but only if
multi-VM is in launch scope. It also asks for an ADR on the security
consequence either way. ADR-016 leaves multi-VM out of v1 ("not built; gas
semantics unresolved").

## Decision

1. **No EVM layer is built for v1.** Nothing here prepares a half-layer that
   could be switched on by accident. The work waits for ADR-016's gate, a
   gas-parity suite between the WASM VM and revm.
2. **When it is built, EVM accounts are "classical security".** Standard EVM
   wallets sign with secp256k1, which a quantum computer breaks. An
   EVM-compatible account on this chain is only as strong as that signature.
   Explorers, wallets, the RPC and `chain/*.json` must label such accounts
   *classical security*. They must never inherit the chain's post-quantum
   label.
3. **The post-quantum path for EVM users is account abstraction.** A smart
   account whose validation calls an ML-DSA verification precompile, so an
   EVM contract can hold funds under a post-quantum key. `crates/smart-account`
   is the native design this would mirror.
4. **Value moving from a classical account to a post-quantum one is allowed.
   The reverse is allowed but labelled.** A chain that silently mixes the two
   security levels gives users a guarantee it does not have.

## Consequences

- Foundry, Hardhat, MetaMask, ethers and viem do not work against this chain
  in v1, and the integration docs say so.
- The Ethereum JSON-RPC compatibility test set is not run: there is nothing to
  run it against.
