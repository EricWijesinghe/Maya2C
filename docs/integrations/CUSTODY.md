# Institutional custody

Master Prompt 17 §2.

## What a key is

A Maya2C account key is a hybrid pair: ML-DSA-65 (FIPS 204) and
SLH-DSA-SHA2-128s (FIPS 205). Both halves sign every transaction, and both
must verify (TX-1). Neither is an elliptic-curve key.

## HSMs

See ADR-022 for the vendor landscape. It was written without live
verification, so check it yourself. An HSM must support **both** ML-DSA-65
and SLH-DSA-SHA2-128s to hold a hybrid account key. Today's PQC HSM firmware
most commonly offers ML-DSA; SLH-DSA support is rarer. Where the HSM cannot
hold both, keep the seed on an air-gapped machine (below).

## MPC is not a drop-in

**Most MPC custody stacks today use elliptic-curve threshold schemes (ECDSA or
EdDSA threshold signing). They do not apply to ML-DSA or SLH-DSA.** A lattice
threshold scheme exists in this tree only as RESEARCH
(`crates/custody-mpc`, feature `threshold-lattice`) and is not audited.
Until one is, **on-chain m-of-n multisig is the supported path**: each
signer holds a whole key and the chain checks m signatures. The multisig
transaction (wire v8, ADR-013) is dark at activation height `u64::MAX` today;
activating it is a governance decision.

## Air-gapped signing

The loop is tested end to end in `crates/node/tests/offline_signing.rs`:

1. **Online** (no key): build the transfer from the sender's address and
   current nonce; export the unsigned frame (`Transaction::to_bytes` with no
   signature).
2. **Offline** (key, no network): decode the frame and show the operator the
   nonce, amount and recipient; sign; export the signed frame.
3. **Online**: submit the signed frame (`send_raw_transaction`). The mempool
   verifies both signatures. A frame altered on the way back is refused
   (tested).

The key is derived from a 32-byte seed
(`crypto::hybrid::signing_key_from_seed`). Back up the seed, not the key.
Hybrid signing is deterministic in both halves, so the same seed and frame
always produce the same signature.
