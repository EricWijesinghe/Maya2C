# 2. Transactions — v0.1.0

Reference: `crates/spec-ref/src/{wire,stf}.rs`. Node: `Transaction::verify`,
`Transaction::sender`, `StateDB::stage_transaction`.

- **TX-1** A transaction authorizes nothing unless **both** its ML-DSA-65 (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205) signatures verify over TX-3's bytes. This is checked before any balance rule, and after the nonce check (STF-1), which is a cheaper refusal of a stale transaction; both orders accept the same transactions. A block containing a transaction that fails it is invalid.
- **TX-2** The sender is derived, never read from the wire: `blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)`. *(positive only: defines a value)*
- **TX-3** Both signatures sign `"custom-l1-node.tx.v4" ‖ chain_tag ‖ io_section ‖ public_key ‖ nonce_u64` (plus the payload encoding for version 6), where `chain_tag` is TX-5's. The keys are inside the signed bytes, so each signature commits to the other scheme's key. Suite-tagged (v7) and multisig (v8) transactions sign under `"custom-l1-node.tx.suite.v2"` and `"custom-l1-node.tx.multisig.v2"`, each followed by `chain_tag` and then their own fields. *(positive only: defines a value)*
- **TX-5** `chain_tag` is the 32-byte id (CON-2) of the genesis block of the one chain a signature is valid on. It is signed, never sent: the wire format (ENC-2) carries no tag, and a node verifies with its own genesis id, so a frame signed for any other chain — another network, or an earlier genesis of the same one — fails TX-1. A node that has not bound its genesis verifies nothing. (ADR-036.)
- **TX-4** Suite-tagged (v7) and multisig (v8) transactions verify only through the height-aware `verify_at`, at or past `SUITE_ENVELOPE_ACTIVATION_HEIGHT` — `0` since ADR-013, so from genesis — under the suite registry's policy; `Transaction::verify`, which has no height, refuses them.

## DEX settlement (ADR-154)

- **DEX-1** Neither a `Swap` nor a `SwapRoute` executes where it sits in the block. Each is staged with its input in escrow, and at the end of the block every pool clears exactly once: one uniform-price batch holding the pool's plain swaps and one leg of every route that crosses it. Pools clear in a topological order of the routes' paths, smallest pair identifier first among the ready ones, so the post-state does not depend on transaction order. *(external vectors: crates/node/tests/dex_fairness_tests.rs)*
- **DEX-2** A route is all or nothing: if its final output is below `min_out`, a pool cannot price one of its legs, or its path would close a cycle with routes of smaller identifier, it is refunded whole and the pools clear as if it had not been staged. That is a no-op, not an error (invariant 7). A route that names a pool twice is an error. *(external vectors: crates/dex/tests/clearing_tests.rs)*
- **DEX-3** A pair's batch holds at most 256 plain swaps and a block at most 64 routes. When either is full, a new intent whose weight — the fee its transaction paid the collector, then the reverse of its identifier; a sealed reveal above every plaintext intent — exceeds the lowest staged weight evicts that intent and its input is refunded; otherwise the new intent is a no-op. *(external vectors: crates/node/tests/dex_fairness_tests.rs)*
- **DEX-4** One sender holds at most 16 of a pair's plain swaps and 4 of a block's routes; a transaction past either cap is an error (`TooManySwaps` for swaps). *(external vectors: crates/node/tests/dex_fairness_tests.rs)*

## Gaps

- TX-4's vectors (`consensus.json`) state the path, not the frame bytes: the reference does not re-implement signatures, so the node builds real v7 (ML-DSA-87) and v8 (2-of-3) frames and checks which call accepts them.
- Typed payloads (channels, DEX, governance) are specified only by the node's code today.
  For DEX this includes the ADR-144 settlement rules: a batch on a pool that
  cannot carry the governed `DexProtocolFeeBps` is refunded, a route through
  it is a no-op, and a book fill pays the governed protocol rate on the
  taker's output (the LP rate never applies to a book trade). These are pinned
  by `crates/node/tests/dex_tests.rs` and `docs/dex.md`, with no rule ID or
  vector yet.
