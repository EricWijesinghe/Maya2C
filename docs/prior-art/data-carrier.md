# Prior art: Data-carrier outputs and inscription-class metaprotocols

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "OP_RETURN 80 byte limit Bitcoin Core 30 removal ordinals inscriptions envelope runes protocol").

Feature: parity row P-22, [ADR-119](../adr/ADR-119-data-carrier.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Bitcoin `OP_RETURN` | A provably unspendable output carrying bytes; the 80-byte relay default was removed in Bitcoin Core 30 (October 2025), with the old `-datacarriersize` options deprecated. Coverage disagrees on the new default (one says ~4 MB bounded by the block, one 100,000 bytes); the release notes and PR #32406 are the primary source. | [CoinDesk](https://www.coindesk.com/tech/2025/06/10/bitcoin-core-30-to-increase-op_return-data-limit-after-developer-debate-concludes), [Unchained](https://unchainedcrypto.com/bitcoin-core-v30-releases-with-expanded-op_return-capacity/), [The Block](https://www.theblock.co/post/353296/bitcoin-community-split-over-op_return-limit-removal-plan-from-core-devs) |
| Ordinals / inscriptions, Runes | Bytes hidden in Taproot witness envelopes because the carrier was capped; the argument that a cap pushes data into more opaque forms is what moved Core 30. Runes and KRC-20 are not covered by the search (noted as not searched). | [Decrypt](https://decrypt.co/324424/bitcoin-core-devs-commit-to-changing-op_return-data-storage-limit-by-october) |
| Kaspa Silverscript covenants (from the parity survey) | Script-level data and covenants. | [kaspa.org/lore](https://kaspa.org/lore) |

## What Maya2C has (checked in repo, 2026-11-12)

- No free-byte field: `TxKind` is a closed typed list (`crates/node/src/core/payload.rs:438-554`); `AttestLegal` carries "a hash, never a document".
- Per-byte burning base fee (`state/fees.rs:1-17`), 5 MiB block cap with first-fit (invariant 33), target-bytes controller (ADR-076).

## How the Maya2C version differs

- A **typed kind** `Data { tag, body }` in the existing payload frame, capped at 64 KiB, with no state effect: indexable by tag rather than hidden in a witness.
- The byte fee burns and the target controller responds to a flood; no separate data discount or surcharge is proposed (Eric's question).
- An authenticated author for free: the transaction's signer.

**Verdict: similar exists (OP_RETURN, inscriptions). Differentiate by the typed, tagged kind and the fee-burn response. No "first" claim.**
