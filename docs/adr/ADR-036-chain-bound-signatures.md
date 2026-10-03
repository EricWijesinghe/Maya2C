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

## Implementation notes (2026-10-03)

- **The txid names no chain.** `Transaction::txid` is
  `blake3("custom-l1-node.txid.v1" ‖ kind ‖ that kind's body ‖ signatures)`,
  with kind `0x00` hybrid, `0x07` suite-tagged, `0x08` multisig (ENC-8). It
  is used to deduplicate within one chain and to build `tx_root`, neither of
  which needs a chain, and keeping it chain-free kept the tag out of every
  consensus path that hashes transactions.
- **The kind's own body, not the hybrid one.** A first cut hashed the hybrid
  body for every kind. A multisig id hashes no approval (any quorum must
  give one spend one id), so two wallets with different policies paying the
  same outputs at the same nonce got the same id and the mempool would have
  dropped one as a duplicate. Caught in review before merge; pinned by
  `multisig_tests::two_wallets_paying_the_same_outputs_at_the_same_nonce_have_different_ids`.
- **The Ledger app shows the genesis.** `apps/ledger-maya2c` parses the v2
  suite domain and the tag, and puts the genesis id on the review screen
  ("Network genesis"). A device that signs for whatever chain it is handed
  would let a request dressed as a testnet transfer spend mainnet funds.
- **`StateDB::bind_chain` is idempotent for the same tag** and refuses a
  different one, so a caller may bind before `Chain::open` binds again.
- **Independent checks.** `spec/tests/encoding.json` pins the signing bytes
  for a stated tag (TX-3, TX-5); `state_transitions.json` has a transfer
  signed for another genesis that must be refused. The node, `spec-ref` and
  the TypeScript verifier (`spec/verifier-ts`) all agree on them.
- **Test helper.** `crates/node/tests/common` tracks, per test thread, the
  genesis of the chain the test opened (`common::open_chain`), so test
  transactions sign for the chain that will verify them; a test that drives a
  bare `StateDB` binds a fixed tag with `common::bind`.

## Open before mainnet (security review, 2026-10-03)

- **Wallets trust their node for the genesis.** A wallet fetches the tag from
  the node it talks to. A malicious node, or anyone in the middle of a
  plain-HTTP RPC URL, can hand it another chain's genesis, and the user then
  signs a transfer valid only there — addresses are the same on every chain.
  The amount, recipient and nonce are still the user's, so nothing beyond
  what they approved moves, but they approve it on a chain they did not
  choose. Fix before mainnet: wallets carry the genesis of each network they
  know (testnet, mainnet), refuse to sign for an unknown one without an
  explicit confirmation, and show the network name and a short fingerprint
  before signing; the Ledger app shows a name for known geneses rather than
  64 hex characters nobody will compare. `get_chain_info` now returns the
  network name next to the genesis, which the wallet can display.
- **Every network needs its own genesis bytes.** The tag is the genesis id,
  so two networks launched from byte-identical genesis blocks share a tag and
  each other's signatures. The mainnet ceremony must produce a genesis that
  differs from every testnet's (it will: different allocations and time), and
  the ceremony runbook checks it against the published testnet ids.
- **A multisig body can be re-encoded under the same id.** Approvals are not
  in the txid (by design, ADR-011), so `tx_root` does not commit to them, and
  `state/preview.rs` keys its kept preview on txids. A variant with other
  approvals reuses the id. No cache of "invalid by id" exists today; key the
  preview on wire hashes, as `state/verified.rs` does, before multisig is
  used on mainnet. Predates this ADR.
