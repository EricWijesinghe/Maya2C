# 2. Transactions — v0.1.0

Reference: `crates/spec-ref/src/{wire,stf}.rs`. Node: `Transaction::verify`,
`Transaction::sender`, `StateDB::stage_transaction`.

- **TX-1** A transaction authorizes nothing unless **both** its ML-DSA-65 (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205) signatures verify over TX-3's bytes. This is checked before any balance rule; a block containing a transaction that fails it is invalid.
- **TX-2** The sender is derived, never read from the wire: `blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)`. *(positive only: defines a value)*
- **TX-3** Both signatures sign `"custom-l1-node.tx.v3" ‖ io_section ‖ public_key ‖ nonce_u64` (plus the payload encoding for version 6). The keys are inside the signed bytes, so each signature commits to the other scheme's key. *(positive only: defines a value)*
- **TX-4** Suite-tagged (v7) and multisig (v8) transactions verify only at or past `SUITE_ENVELOPE_ACTIVATION_HEIGHT`, which is `u64::MAX`: today every one is rejected.

## Gaps

- TX-4 has no vector (the verifier would need a v7 frame).
- Typed payloads (channels, DEX, governance) are specified only by the node's code today.
