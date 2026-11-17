# 2. Transactions — v0.1.0

Reference: `crates/spec-ref/src/{wire,stf}.rs`. Node: `Transaction::verify`,
`Transaction::sender`, `StateDB::stage_transaction`.

- **TX-1** A transaction authorizes nothing unless **both** its ML-DSA-65 (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205) signatures verify over TX-3's bytes. This is checked before any balance rule, and after the nonce check (STF-1), which is a cheaper refusal of a stale transaction; both orders accept the same transactions. A block containing a transaction that fails it is invalid.
- **TX-2** The sender is derived, never read from the wire: `blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)`. *(positive only: defines a value)*
- **TX-3** Both signatures sign `"custom-l1-node.tx.v4" ‖ chain_tag ‖ io_section ‖ public_key ‖ nonce_u64` (plus the payload encoding for version 6), where `chain_tag` is TX-5's. The keys are inside the signed bytes, so each signature commits to the other scheme's key. Suite-tagged (v7) and multisig (v8) transactions sign under `"custom-l1-node.tx.suite.v2"` and `"custom-l1-node.tx.multisig.v2"`, each followed by `chain_tag` and then their own fields. *(positive only: defines a value)*
- **TX-5** `chain_tag` is the 32-byte id (CON-2) of the genesis block of the one chain a signature is valid on. It is signed, never sent: the wire format (ENC-2) carries no tag, and a node verifies with its own genesis id, so a frame signed for any other chain — another network, or an earlier genesis of the same one — fails TX-1. A node that has not bound its genesis verifies nothing. (ADR-036.)
- **TX-4** Suite-tagged (v7) and multisig (v8) transactions verify only through the height-aware `verify_at`, at or past `SUITE_ENVELOPE_ACTIVATION_HEIGHT` — `0` since ADR-013, so from genesis — under the suite registry's policy; `Transaction::verify`, which has no height, refuses them.

### Stealth addresses (ADR-102) — dark, `STEALTH_ACTIVATION_HEIGHT = u64::MAX`

Reference: `crates/crypto-pq/src/stealth.rs`. Not yet a transaction kind or a wire version; these rules fix the derivation so the later frame cannot drift from it.

- **ST-1** For an ML-KEM-768 ciphertext `ct` (1088 bytes) and shared secret `ss` (32 bytes), `k = SHAKE256("maya2c/stealth/v1" ‖ ss ‖ ct)` truncated to 64 bytes; the view tag is `k[0]` and the tweak is `k[32..64]`. *(positive only: defines a value)* (external vectors: crates/crypto-pq/tests/vectors/stealth/derivation-v1.json)
- **ST-2** The one-time address is `P = BLAKE3-derive-key("maya2c/stealth/addr/v1", A ‖ tweak)`, where `A` is the recipient's suite address. *(positive only: defines a value)* (external vectors: crates/crypto-pq/tests/vectors/stealth/derivation-v1.json)
- **ST-3** An announcement is exactly 1089 bytes, `ct ‖ tag`; any other length is refused. A wallet detects a payment only when the recomputed tag equals `tag` **and** an output of the same transaction equals `P`; a flipped tag or a tampered `ct` is not detected. (external vectors: crates/crypto-pq/tests/vectors/stealth/derivation-v1.json)
- **ST-4** A stealth spend signs `"maya2c/stealth/spend/v1" ‖ tweak ‖ message` under `spend_pk` and authorizes exactly `P = ST-2(suite_address(suite, spend_pk), tweak)`; a swapped tweak or a plain suite signature over `message` does not verify. (external vectors: crates/crypto-pq/tests/stealth_tests.rs)
- **ST-5** No stealth announcement or spend is admitted below `STEALTH_ACTIVATION_HEIGHT`, which is `u64::MAX`; an upgrade entry naming `FeatureSet::STEALTH` (bit 10) is refused until the v10 frame exists. (external vectors: crates/node/src/upgrade.rs)

## Gaps

- TX-4's vectors (`consensus.json`) state the path, not the frame bytes: the reference does not re-implement signatures, so the node builds real v7 (ML-DSA-87) and v8 (2-of-3) frames and checks which call accepts them.
- Typed payloads (channels, DEX, governance) are specified only by the node's code today.
