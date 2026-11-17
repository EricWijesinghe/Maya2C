# Prior art: Bitcoin anchoring of chain checkpoints

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "Bitcoin OP_RETURN checkpoint anchoring OpenTimestamps Komodo delayed proof of work Babylon Stacks"; "Komodo delayed proof of work dPoW notarization Bitcoin; Liquid Kapow checkpoint OP_RETURN; OpenTimestamps calendar fees who pays" — Liquid's Kapow surfaced in the first query's gist; OpenTimestamps fee model did not surface and is marked thin).

Feature: parity row P-33, [ADR-126](../adr/ADR-126-bitcoin-checkpoint-anchoring.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Komodo dPoW | 64 elected notary nodes write a Komodo block hash into Bitcoin via OP_RETURN roughly every ten minutes, then back-notarize into Komodo and its child chains; critics call it Bitcoin checkpoints under another name and dispute "full Bitcoin security" marketing. | [dPoW whitepaper](https://github.com/KomodoPlatform/komodo/wiki/Delayed-Proof-of-Work-(dPoW)-Whitepaper), [dPoW repo](https://github.com/komodoplatform/dpow), [docs: confirmations](https://docs.komodoplatform.com/komodo/dPOW-conf.html), [critique](https://medium.com/@JEhrenhofer/no-dpow-isnt-a-perfect-solution-7e6e43cc20be) |
| Babylon | A vigilante relayer packages checkpoints (compressed BLS multisignatures, because raw commit data runs to kilobytes) into OP_RETURN transactions. | [Babylon Substack](https://babylonchain.substack.com/p/checkpointing-babylon-to-btc), [Babylon docs](https://docs.babylonlabs.io/guides/research/btc_timestamping) |
| Stacks | Each Stacks block hash is written into a Bitcoin transaction. | [spark.money glossary](https://www.spark.money/glossary/data-anchoring) |
| Liquid (Kapow checkpoints) | Signers post checkpoint transactions recording the Liquid block hash and height in an OP_RETURN output; nodes reject blocks that do not match, and delayed checkpoints lengthen finality rather than fork. | [gist](https://gist.github.com/lukechilds/951ec0e32c818f5b24f052883441d6cc) |
| OpenTimestamps | Calendar servers batch hashes into a Merkle tree whose root goes into one Bitcoin transaction; portable `.ots` proofs verify against Bitcoin without the service. | [spark.money](https://www.spark.money/tools/bitcoin-timestamp-server-explained), [IETF SCITT time-anchor draft](https://www.ietf.org/archive/id/draft-fassbender-scitt-time-anchor-03.html) |
| LayerEdge | Bitcoin anchoring of aggregated proofs. | [docs](https://docs.layeredge.io/docs/architecture/bitcoin-anchoring) |
| OP_RETURN policy | The 80-byte limit was relay policy, not consensus. | [spark.money glossary](https://www.spark.money/glossary/data-anchoring); newer Core defaults are looser (from knowledge, not re-searched) |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/btc-spv` (RESEARCH, `features.toml:3043`): headers, retarget, most-work fork choice, Merkle inclusion proofs; "nothing in the node calls it" (`lib.rs:21-22`).
- `crates/btc-bridge` (RESEARCH, `features.toml:2364`): parses Bitcoin transactions, requires exactly one OP_RETURN with a 32-byte payload (`lib.rs:4-12`).
- Checkpoints served by `get_checkpoint` (`crates/node/src/rpc/server.rs:545`); ADR-094 (branch `feat/notary`) anchors file hashes *into* Maya2C, the other direction.
- No Bitcoin wallet, signer or fee logic in the tree.

## How the Maya2C version differs

- **No consensus dependency**: unlike Komodo's back-notarization and Liquid's rejection rule, no Maya2C node reads an anchor; the verifier is an off-chain tool on `btc-spv`.
- The anchored value is the hash of the full checkpoint encoding (block id, epoch, height, ordered attestations), so the anchor dates *who attested*, not the block id alone.
- The publisher is a disclosed, optional party that pays Bitcoin fees itself; absent by default (invariant 11's shape).

**Verdict: similar exists (Komodo, Babylon, Stacks, Liquid, OpenTimestamps). Differentiate by the strict no-consensus-dependency rule and the checkpoint-encoding commitment. No "first" claim.**
