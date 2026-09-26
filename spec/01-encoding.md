# 1. Encoding — v0.1.0

All integers are little-endian and fixed-width. Every variable-length
section carries an explicit `u64` count. A frame is consumed exactly.

Reference: `crates/spec-ref/src/wire.rs`. Node: `crates/node/src/core/transaction.rs`.

## Transaction frame (plain transfer, version 5)

```
u8   version                 = 5
u64  input_count             (always 0 in the account model)
     inputs[input_count]     prev_tx[32] ‖ index_u32        (36 B each)
u64  output_count
     outputs[output_count]   amount_u64 ‖ recipient[32]     (40 B each)
     public_key[1984]        ml_dsa_65_pk[1952] ‖ slh_dsa_sha2_128s_pk[32]
u64  nonce
u8   signature_flag          0 = unsigned, 1 = signed
     signature[11165]        present iff flag = 1: ml_dsa_65_sig[3309] ‖ slh_dsa_sig[7856]
```

## Rules

- **ENC-1** The first byte is the wire version. Versions 1–4 (single-signature eras) are rejected by name; 5 is a transfer, 6 a transfer with a typed payload, 7 suite-tagged, 8 multisig; any other value is rejected.
- **ENC-2** A version-5 frame is laid out exactly as above, and re-encoding a decoded frame reproduces its bytes (one encoding per transaction).
- **ENC-3** The input/output section is `u64 count ‖ inputs ‖ u64 count ‖ outputs`, each element fixed-width.
- **ENC-4** The signature flag is 0 or 1; any other value is rejected.
- **ENC-5** A frame with bytes missing or bytes left over is rejected.
- **ENC-6** A collection count above 65,536 is rejected before any allocation.
- **ENC-7** A collection count whose elements cannot fit in the remaining input is rejected before any allocation.
- **ENC-8** A version-5 transaction's id is `blake3(signing_bytes ‖ ml_dsa_sig ‖ slh_dsa_sig)` (TX-3), the signature omitted when the frame is unsigned. Hashing the signature commits the id to one authorization; that is sound only because both schemes sign deterministically, so one payload under one key pair has one id. (Multisig v8 ids deliberately exclude approvals.) *(positive only: defines a value)*

## Gaps

- Version 6 payload encodings (`TxKind`) have no vectors.
- Versions 7 and 8 are active from genesis (`SUITE_ENVELOPE_ACTIVATION_HEIGHT = 0`, ADR-013) and have no vectors yet.
- Block and header encodings: see [05-consensus.md](05-consensus.md).
