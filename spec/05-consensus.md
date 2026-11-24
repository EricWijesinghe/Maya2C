# 5. Consensus — v0.1.0

The node runs one of two engines, chosen by its genesis: **DAG-BFT**
(Narwhal certification, Bullshark commit, `crates/node/src/consensus/bft/`)
when the genesis names a `bft` committee, and Nakamoto **proof of work**
otherwise. Under DAG-BFT a block is derived from certificates and no work is
verified, so CON-5, CON-6 and CON-7 apply to proof-of-work chains only,
CON-9 to DAG-BFT chains only, and CON-1 to CON-4 and CON-8 to both.

Node: `crates/node/src/core/block.rs`, `crates/node/src/consensus/`.

- **CON-1** A header is 145 bytes: `prev_hash[32] ‖ state_root[32] ‖ timestamp_u64 ‖ nonce_u64 ‖ difficulty_target[32] ‖ tx_root[32] ‖ version_u8`. *(positive only: defines a value)*
- **CON-2** A block's id is BLAKE3 in derive-key mode with context `"custom-l1-node header id v1"` over the 145 header bytes. *(positive only: defines a value)*
- **CON-3** `tx_root` is ROOT-3's tree over leaves `blake3_derive_key("custom-l1-node tx leaf v1", txid)`; a block whose header disagrees is invalid.
- **CON-4** A block's declared `state_root` must equal the root its execution produces (invariant 24); otherwise it is invalid and nothing is written.
- **CON-5** The declared difficulty target must equal the retarget rule's output for that height (every 100 blocks, 15 s target spacing), the window measured between the median times (CON-11's median) of its last and first blocks, not between two single timestamps; otherwise invalid.
- **CON-6** The proof-of-work hash for the block's height must meet its target; otherwise invalid.
- **CON-7** The active chain is the one with the greatest cumulative work, a target's work being `2^256 ÷ (target + 1)`. *(positive only: a choice between valid branches, never a refusal)*
- **CON-8** A block at or below the prune horizon is refused.
- **CON-9** Under DAG-BFT a block's `difficulty_target` equals its parent's, so every block carries the genesis target; otherwise invalid. A DAG-BFT genesis must declare `difficulty_bits` and `pow_limit_bits` of 0 (the unlimited target). There is no retarget and no activation pin: the target certifies nothing here, and a retarget on one-second blocks drove cumulative work to 2^256 − 1 within about 12,500 blocks, which stopped the chain (ADR-035).
- **CON-10** A header's `version` is 0; any other value is invalid, and a header of another length is not a header (ADR-063).
- **CON-11** On a chain whose target is retargeted (CON-5), a block's timestamp is later than the median of the timestamps of its parent and up to ten earlier ancestors (the upper middle when fewer than eleven exist); otherwise invalid. A DAG-BFT chain (CON-9) does not apply this rule: it has no retarget to steer, and its blocks can come faster than once a second (ADR-063).
- **CON-12** A block's wire size is `145 + 8 + Σ(8 + len)` bytes, where `len` is each transaction's encoding, and it is at most `MAX_BLOCK_BYTES` = 5 MiB (5,242,880) (ADR-072, provisional). A block over the cap is invalid, whoever built it. A builder admits transactions first-fit, in order: one that would take the block over the cap is passed over and the ones after it are still considered; a transaction that does not execute costs no room. The admitted indices are a function of the order and the cap alone, so every node makes the same cut. *(provisional value: pending measurement on the launch host)*
- **CON-13** *(positive only: CON-13 selects payloads and never refuses one; a refusal is CON-12's `BlockTooLarge`)* A DAG-BFT builder decodes a committed sub-DAG's batch payloads within `DECODE_BUDGET` = 3/2 × `MAX_BLOCK_BYTES` (7,864,320), shared fairly among authors by deficit round-robin.
  - Each author's queue is its payloads in commit order. Authors are taken in ascending `ValidatorId` (the committee index in each vertex).
  - Every turn adds `Q = max(1, DECODE_BUDGET / n)` to each author still waiting, where `n` is the committee size. The divisor is the committee size, not the number of active authors, so an honest author's first-turn credit does not depend on how many authors are active.
  - An author admits payloads from the front of its queue while the next one fits both its deficit and the budget left. A payload identical to one already admitted is skipped without a charge or a decode.
  - An author finishes when its queue is empty, or when its next payload is larger than the budget left. Its admitted set is therefore a prefix of its queue: nothing is passed over.
  - A finished author's unused deficit is dropped. The budget left keeps going to the others.
  - Admitted payloads are emitted in commit order, so execution order is the commit order restricted to the admitted set.
  - The bytes charged are raw payload bytes, per author. Payloads not admitted are not in the block and stay with their proposers.
  - Decode work per block is at most `DECODE_BUDGET`. Removing repeats compares raw bytes and decodes nothing (ADR-072).
- **CON-18** A DAG-BFT vertex's digest, the value every proposal signature, vote and parent link refers to, is `blake3("maya2c/dag-bft/vertex/v3" ‖ epoch_u64 ‖ round_u64 ‖ author_u32 ‖ timestamp_ms_u64 ‖ parents_root ‖ batch_root)`, where `parents_root = blake3_derive_key("maya2c dag-bft vertex parents v1", n_u64 ‖ parent*n)` and `batch_root = blake3_derive_key("maya2c dag-bft vertex batch v1", m_u64 ‖ blake3(tx)*m)`, all integers little-endian. Equivocation evidence (`ReportEquivocation`) is two signed vertex headers, `0x01 ‖ epoch ‖ round ‖ author_u32 ‖ timestamp_ms ‖ parents_root ‖ batch_root ‖ sig_len_u64 ‖ sig`, so a report is the same size whatever the two vertices carried: double-signing stays provable on chain at any block size (ADR-155, NEW-1). A header of another version is refused. *(positive only: defines a value; the refusal is a decode error)*
- **CON-19** A certificate record is `0x01 ‖ vertex ‖ k_u64 ‖ (voter_u32 ‖ sig_len_u64 ‖ sig)*k`, with `vertex = epoch ‖ round ‖ author_u32 ‖ timestamp_ms ‖ n_u64 ‖ parent*n ‖ m_u64 ‖ (len_u64 ‖ tx)*m`. The first byte is the record's format version; a record of any other version is refused, never read as version 1. Every validator id on the DAG-BFT wire (frame sender and addressee, vertex author, voter) is four bytes; this build's committee index is 16 bits, so an id above 65,535 is refused at decode, and a wider committee later changes no encoding (ADR-155, C2, NEW-7). The value `0xFFFFFFFF` is reserved: in a frame's sender it means "no sender" (an observer) and in its addressee "every validator"; it is never a validator id, so a vertex author or voter of `0xFFFFFFFF` is refused at any committee width. `0x0000FFFF` is the ordinary id 65,535. The gossip frame version is 2.

## Vectors and gaps

CON-1 to CON-3 are in `spec/tests/headers.json`, CON-4 in
`state_transitions.json` (a declared root that matches and one that does
not) and CON-5 to CON-8, CON-10, CON-11, CON-12, CON-18 and CON-19 in `consensus.json`. CON-6's vectors pin the
comparison of a hash with a target; the proof-of-work hash itself
(`argonblake-pow`) has no language-neutral vector.

DAG construction, the commit rule, finality and fork choice under DAG-BFT
are specified only by the node's code and `crates/dag-bft`, and have no rule
IDs yet.

Catch-up by certificate replay (ADR-052) is specified by its ADR and the node,
with no rule IDs and no conformance vectors yet. Its wire rule: `get_certificates`
serves at most 256 records per page and about 8 MiB; a client refuses a larger
page, a page that makes no progress, and any certificate without a quorum
(`n − f` by the engine's rule) of valid signatures over its digest from the
epoch's committee. A page must end where its records end, and a finished epoch
is complete only if the replayed history contains its attested end (the epoch's
final checkpoint, a quorum-signed block id at a height). A peer's `done` is not
evidence of completeness. Vectors: NOT YET (gap). Node: `crates/node/src/consensus/bft/certsync.rs`.
