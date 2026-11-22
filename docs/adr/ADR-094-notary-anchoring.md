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

Verification scans blocks for the transaction, checks each served block's
transactions against the `tx_root` in the header its id hashes, checks that id
is the one the node named, checks the stamp's signature against the chain tag
the node reports, and reads the newest committee checkpoint for
finality. The node's RPC is not extended.

## Consequences

- No consensus, wire or state change. Nothing new reaches the node.
- Stamps overwrite the node's record for their schema. The transaction in its
  block is the proof, so receipts keep the txid.
- Verify is linear in chain length, because there is no lookup by txid. A
  `get_transaction` RPC would remove that cost. It is a post-genesis change.
- Finality trusts the node's checkpoint. Verifying committee signatures is the
  next step and is not in this change. Verify counts a checkpoint only when
  the block it is served at the checkpoint's height has the id the checkpoint
  names, so a node on another branch does not report `final`.
- Block time under DAG-BFT is the median of the ordered sub-DAG's vertex
  times, never earlier than the parent's; it is honest only under the BFT
  assumption.
- `setup` and `stamp` report success only once the sender's nonce has passed
  the one they used (two-minute limit), so back-to-back calls do not share a
  nonce.
- Verify pins the genesis (`--genesis`, follow-up PR #620), so the chain tag
  the signature is checked against is no longer the node's word.
- The block time and height are the node's word: a dishonest node can place a
  real stamp in an invented block with an earlier time. Until checkpoint
  signatures and the `prev_hash` chain from the stamp block up to the
  checkpoint are verified, use a node you run. That check is the fix, and it
  comes with the committee-signature step above.
- One earlier manual run (before the devnet test existed) never saw the stamp
  in a block. It was not reproduced; the likely cause, found by reading the
  code, is the back-to-back nonce reuse fixed above. The status rests on CI:
  job 132953260484 (`PASS [49.961s]`) and run 44011408863 on master's merge
  commit e06afef2, job 133801563810 (`PASS [50.993s]
  maya2c-cli::notary_devnet_tests
  a_stamp_lands_in_a_block_and_verifies_with_a_receipt`), plus 10 of 10 local
  runs on 2026-11-21 (PR #620, DECISIONS.md).
- BLAKE3 is plain (not domain-separated), so standard tools reproduce the hash.
