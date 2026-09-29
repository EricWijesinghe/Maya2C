# ADR-036: Transaction signatures commit to the chain's genesis

**Status:** Accepted (2026-09-30). A transaction-validity rule, and mainnet
launch gate 1 (`docs/mainnet-v1-plan.md`).
**Date:** 2026-09-30

## Context

`Transaction::signing_bytes` covered a fixed domain (`custom-l1-node.tx.v3`)
and the transaction's own fields, and nothing that named a network. A
transfer signed on one Maya2C chain therefore verified on every other chain
where the sender's account had the same nonce. A testnet transfer could be
replayed on mainnet by anyone who had seen it; so could a transfer from a
chain that was reset, like maya-testnet-1 after the halt at 12,530 (ADR-035).
This is the class Ethereum closed with EIP-155.

A chain id string would not have been enough. maya-testnet-1 has already had
two geneses under one name.

## Decision

**Every signature commits to the genesis block id of the chain it is valid
on.**

- `ChainTag` is the 32-byte genesis block id (CON-2's header hash of block 0).
- The signed bytes of every authorization kind start with a bumped domain and
  then the tag: hybrid (`custom-l1-node.tx.v4`), suite-tagged and multisig
  (each domain bumped by one version), followed by the fields exactly as
  before. Bumping the domain means no signature over the old encoding can
  ever verify again.
- The wire format does not change: the tag is signed, never sent. A
  transaction is no bigger and its fee does not change. A frame signed for
  one chain fails signature verification on any other.
- The context-free `sign`/`verify` are gone. Every signer and verifier names
  the chain (`sign(key, &tag)`, `verify(&tag)`, `verify_at(height, policy,
  &tag)`), so a forgotten call site is a compile error, not a silent hole.
- The node's `StateDB` learns its tag once, when `Chain::open` binds it to a
  genesis, and refuses to verify before that (fail closed). The verified-
  signature cache folds the tag into its key.
- Wallets and SDKs fetch the tag while online (`get_chain_info`, on the
  gateway's allowlist) and sign offline with it, the way they already fetch
  fee terms.

## Consequences

- **Every existing signature is invalid under the new rule**, so
  maya-testnet-1 restarts from a new genesis again. Mainnet has not launched,
  so mainnet is replay-protected from block 0 and never needs an activation
  height for this.
- Every signing implementation must change in step: the node core, the
  desktop wallet core, `l1-wallet`, the faucet, the pool service, the CLI, the
  HTLC watcher and `sdks/sdk-wasm` (a second, pure-Rust implementation for the
  browser, kept in step by its parity tests). Spec TX vectors are regenerated
  from `spec-ref`.
- Offline signing gains one input (the tag) alongside the nonce and fee terms
  it already needed.
