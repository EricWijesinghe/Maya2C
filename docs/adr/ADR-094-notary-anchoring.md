# ADR-094: Notary anchors a file hash in an existing transaction type

Status: accepted (tooling, off-chain). Catalog item `notary`.

## Context

The ecosystem catalog lists `notary`: prove that a document existed at a
time, without publishing the document. The chain must carry a commitment,
and the node is frozen until genesis, so the first question is whether a
transaction can carry 32 bytes today.

It can. Two options were found:

- A plain transfer has no payload section, and its outputs are addresses.
  Neither can hold a commitment without moving value.
- `AnchorAttestation` (tag 34) carries a 32-byte root and a schema label. It
  requires the sender to hold a DID, which `RegisterDid` creates. The node
  stores one record per (issuer, schema), and the transaction stays in its
  block.

## Decision

The notary uses `AnchorAttestation` with schema `notary-v1`, signed under the
wallet's own DID. The root is the BLAKE3 hash of the file, computed on the
user's machine. The file never leaves it.

Verification scans blocks for the transaction, checks the block's transaction
root against its header, and reads the newest committee checkpoint for
finality. The node's RPC is not extended.

## Consequences

- No consensus, wire or state change. Nothing new reaches the node.
- Stamps overwrite the node's record for their schema. The transaction in its
  block is the proof, so receipts keep the txid.
- Verify is linear in chain length, because there is no lookup by txid. A
  `get_transaction` RPC would remove that cost. It is a post-genesis change.
- Finality trusts the node's checkpoint. Verifying committee signatures is the
  next step and is not in this change.
- BLAKE3 is plain (not domain-separated), so standard tools reproduce the hash.
