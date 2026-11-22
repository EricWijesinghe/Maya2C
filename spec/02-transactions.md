# 2. Transactions — v0.1.0

Reference: `crates/spec-ref/src/{wire,stf}.rs`. Node: `Transaction::verify`,
`Transaction::sender`, `StateDB::stage_transaction`.

- **TX-1** A transaction authorizes nothing unless **both** its ML-DSA-65 (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205) signatures verify over TX-3's bytes. This is checked before any balance rule, and after the nonce check (STF-1), which is a cheaper refusal of a stale transaction; both orders accept the same transactions. A block containing a transaction that fails it is invalid.
- **TX-2** The sender is derived, never read from the wire: `blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)`. *(positive only: defines a value)*
- **TX-3** Both signatures sign `"custom-l1-node.tx.v5" ‖ chain_tag ‖ version ‖ suite ‖ io_section ‖ public_key ‖ nonce_u64` (plus the payload encoding for version 6), where `chain_tag` is TX-5's and `version ‖ suite` TX-6's. The keys are inside the signed bytes, so each signature commits to the other scheme's key. Suite-tagged (v7) and multisig (v8) transactions sign under `"custom-l1-node.tx.suite.v3"` and `"custom-l1-node.tx.multisig.v3"`, each followed by `chain_tag`, TX-6's header and then their own fields. *(positive only: defines a value)*
- **TX-5** `chain_tag` is the 32-byte id (CON-2) of the genesis block of the one chain a signature is valid on. It is signed, never sent: the wire format (ENC-2) carries no tag, and a node verifies with its own genesis id, so a frame signed for any other chain — another network, or an earlier genesis of the same one — fails TX-1. A node that has not bound its genesis verifies nothing. (ADR-036.)
- **TX-4** Suite-tagged (v7) and multisig (v8) transactions verify only through the height-aware `verify_at`, at or past `SUITE_ENVELOPE_ACTIVATION_HEIGHT` — `0` since ADR-013, so from genesis — under the suite registry's policy; `Transaction::verify`, which has no height, refuses them.
- **TX-6** Every signed message carries, right after `chain_tag`, the frame's wire version (ENC-1) and the suite the signature is under: `0x30` for versions 5 and 6, the frame's suite byte for version 7, and `0x00` for version 8, whose keys each name their suite inside the signed policy. A signature is therefore bound to one version's reading of the bytes: one made for a v5 transfer does not verify a v7 suite-`0x30` frame with the same key, outputs and nonce, nor the reverse. The wire version also selects the hash a frame's id is taken under, so a later id hash is a new wire version, not a re-reading of an old one (ADR-143). *(positive only: defines a value)*

## Gaps

- TX-4's vectors (`consensus.json`) state the path, not the frame bytes: the reference does not re-implement signatures, so the node builds real v7 (ML-DSA-87) and v8 (2-of-3) frames and checks which call accepts them.
- TX-6's cross-version refusal is pinned by real signatures in `crates/node/tests/signed_header_tests.rs`, not by `spec/tests/`: the reference does not re-implement signatures. The header's bytes are in `encoding.json`'s `signing_bytes_blake3`.
- Typed payloads (channels, DEX, governance) are specified only by the node's code today.
  For DEX this includes the ADR-144 settlement rules: a batch on a pool that
  cannot carry the governed `DexProtocolFeeBps` is refunded, a route through
  it is a no-op, and a book fill pays the governed protocol rate on the
  taker's output (the LP rate never applies to a book trade). These are pinned
  by `crates/node/tests/dex_tests.rs` and `docs/dex.md`, with no rule ID or
  vector yet.
