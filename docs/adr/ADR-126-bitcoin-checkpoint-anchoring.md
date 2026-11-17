# ADR-126: Bitcoin anchoring of checkpoints — a disclosed publisher writes OP_RETURN, an off-chain verifier reads it, consensus never looks

**Status:** Proposed, 2026-11-12. Off-chain tooling (W3). No node, wire or state change.
**Date:** 2026-11-12
**Related:** parity row P-33, [prior-art/bitcoin-anchoring.md](../prior-art/bitcoin-anchoring.md), [ADR-038](ADR-038-attested-checkpoints.md), ADR-089 (finality light client, branch `feat/light-client-finality`), ADR-094 (notary, branch `feat/notary`), [ADR-125](ADR-125-succinct-chain-state-proof.md); invariants 11, 24, 26

## Context

A Maya2C checkpoint is a block id with a quorum of its epoch committee's
attestations (`crates/node/src/consensus/bft/attest.rs:171-215`), served by
`get_checkpoint` (`crates/node/src/rpc/server.rs:545`). Finality rests on
`n − f` honest committee members (ADR-038, ADR-040). What a checkpoint
cannot do is prove, to someone who distrusts the committee, that it was
the chain's view *at a time* and that no other checkpoint for the same
height was ever shown to anyone else. Komodo's dPoW, Babylon, Stacks and
Liquid's Kapow write their chain's hash into Bitcoin for that reason
(prior-art file): Bitcoin's work is a public, hard-to-rewrite clock.

What this tree has:

- `crates/btc-spv` (RESEARCH, working, `features.toml:3043`): Bitcoin
  headers, retarget, most-work fork choice, Merkle inclusion proofs
  (`lib.rs:1-22`); "nothing in the node calls it".
- `crates/btc-bridge` (RESEARCH, `features.toml:2364`): parses a Bitcoin
  transaction, requires "exactly one `OP_RETURN`" and a 32-byte payload
  (`lib.rs:4-12`). The parser and the OP_RETURN rule exist.
- ADR-094 (`feat/notary`): anchoring in the *other* direction, a file hash
  into Maya2C via `AnchorAttestation` (tag 34, `payload.rs:150`).

**Findings that contradict the brief.**

1. The parity row lists invariant 11 ("the oracle is optional and absent
   by default") and 26 ("blocks and state commit in one batch"). Neither
   is touched by anchoring *to* Bitcoin: nothing is read from Bitcoin by
   consensus, and nothing is written to state. The invariants that matter
   are 24 (what a checkpoint commits to) and the ADR-038 quorum rule.
   Invariant 11 is relevant in one way: the publisher is a trusted party
   for *timestamping*, and like the oracle it must be absent by default.
2. "Publisher + verifier" are both off-chain. A verifier inside consensus
   would make block validity depend on Bitcoin's chain (reorgs, fee
   spikes, a publisher that stops), which is the Komodo criticism in the
   prior-art file and a liveness dependency this chain does not take.
3. There is no Bitcoin wallet, signer or fee logic in this tree.
   `btc-bridge` parses transactions; it does not build or sign them.

## Decision

**A disclosed publisher — a named operator running `maya2c-anchor` —
writes `OP_RETURN ‖ "M2C1" ‖ BLAKE3(checkpoint encoding)` (36 bytes) into
Bitcoin once per `ANCHOR_EVERY` epochs, paying Bitcoin fees from its own
wallet. A verifier — `maya2c-anchor verify`, built on `crates/btc-spv` —
takes a Maya2C checkpoint and a Bitcoin SPV proof and reports the Bitcoin
block height and median-time-past under which that checkpoint's hash was
buried. Consensus reads none of it; a Maya2C node runs without a
publisher, and a chain with no anchors is the same chain.**

### What is anchored

The hash of the checkpoint's canonical encoding (block id, epoch, height,
the ordered signature map, `attest.rs:171-180`), not the block id alone:
an anchor then commits to *who attested*, so a later dispute about the
committee's view at that height has a Bitcoin-dated witness. The
preimage is the checkpoint itself, which anyone can fetch from
`get_checkpoint` or from the anchor publisher's archive; a verifier that
has it recomputes the hash.

Payload: `0x6a` `OP_PUSHBYTES_36` `"M2C1"` `hash(32)`. 36 bytes is inside
the 80-byte `OP_RETURN` relay policy every searched source treats as the
conservative bound (the prior-art file notes it is relay policy, not
consensus, and that newer Bitcoin Core defaults are looser).

### The publisher

- A binary `bins/maya2c-anchor`, off the node path, that polls
  `get_checkpoint`, keeps the last anchored height, and every
  `ANCHOR_EVERY = 1` epoch builds, signs and broadcasts one transaction
  through a Bitcoin node the operator names (`bitcoind` RPC). Keys are
  the operator's, typed and zeroized in the process (Standing Order 5),
  never in this repository.
- **Disclosed**: the publisher's Bitcoin address, the operator's name
  and the anchoring cadence are published in `docs/ops/anchoring.md` and
  on the status page. More than one publisher may run; each is listed.
  No publisher has any authority: an anchor that disagrees with the
  committee's checkpoint is evidence against the committee, not a rule
  the chain applies.
- **Who pays**: the publisher. The ADR assumes a foundation-run
  publisher for the chain's own checkpoints. Reimbursement from the
  treasury (ADR-111) is an economics question below, not a design here.
  At one anchor per epoch the cost is one Bitcoin transaction per epoch
  at whatever the fee market charges; no figure is stated (Standing
  Order 2).

### The verifier

`maya2c-anchor verify --checkpoint <file> --btc-proof <file>` checks:

1. the checkpoint verifies under its committee (ADR-089's
   `FinalityClient` when merged; `Checkpoint::verify` through the node
   crate until then);
2. the Bitcoin transaction contains exactly one `OP_RETURN` whose payload
   is `"M2C1" ‖ BLAKE3(checkpoint)` (the `btc-bridge` parser's rule,
   `lib.rs:8-12`, lifted into a shared function);
3. the transaction's Merkle proof places it under a header on the
   most-work chain `btc-spv` tracks from a stated set of headers, buried
   `--min-confirmations` deep (the policy is the caller's, as
   `btc-spv/src/lib.rs:11-19` says of deposits).

Output: the Bitcoin height, median-time-past and depth, or the refusal.
The verifier is also what an auditor runs over the publisher's archive
to check that every anchor matches a checkpoint the committee signed.

### What this is not

- Not finality. A checkpoint is final by the committee's quorum, before
  and without any anchor.
- Not a reorg defence. Maya2C's DAG-BFT does not fork under `f < n/3`;
  anchoring adds a dated witness, as Liquid's Kapow and Babylon's
  timestamping do, not a fork-choice rule.
- Not a bridge. `btc-bridge`'s lock/mint stays RESEARCH and unrelated.
- Not a consensus input, ever. A future ADR that wants a node to refuse
  a block because of a Bitcoin anchor is a different design with its own
  invariant review; this one forbids it.

## Alternatives considered

- **Anchor every checkpoint (every block).** One Bitcoin transaction per
  2 s block is a fee bill with no reader; an epoch is the committee's
  unit and the handoff is what a dispute is about.
- **OpenTimestamps calendars instead of a publisher.** Free, batched into
  a Merkle tree by a calendar server, verified with `.ots` proofs. It is
  the right tool for a document; for a chain's checkpoints the publisher
  wants a known address so an auditor can list every anchor. The
  verifier accepts `.ots` proofs as a second route in a later change; the
  prior-art file records the option.
- **A consensus-visible anchor record** (Komodo's back-notarization).
  Puts Bitcoin on the block-validity path. Rejected (finding 2).
- **Anchor the ADR-125 chain proof instead of the checkpoint.** The proof
  changes with the prover; the checkpoint is what the committee signed.
  A proof's hash may be anchored additionally once S1 exists.
- **Do nothing.** A committee key compromise after the fact can then
  rewrite history for anyone who was offline; a Bitcoin-dated witness
  costs one transaction per epoch and takes that away.

## Consequences

- One new binary, one shared OP_RETURN parser lifted from `btc-bridge`,
  one ops document. No node change. `features.toml` gains a `bitcoin-anchoring`
  subsystem (extended, REAL once the verifier's tests exist, off-chain).
- The publisher is a trusted timestamper and is named as one. If it
  stops, nothing on the chain changes and the status page shows the last
  anchor.
- Bitcoin fees are an operating cost somebody pays in BTC. The tree holds
  no BTC and no key.

## Invariants touched

- **11**: the publisher is optional and absent by default; a chain with
  no anchors has the same state root. Mirrors the oracle rule without
  being governed by it.
- **24**: unchanged; the anchored hash is of a checkpoint whose block id
  already covers the block.
- **26**: not touched; nothing is written to state.
- Standing Order 6: the publisher broadcasts Bitcoin transactions that
  cost money; it never runs from CI or an agent without `APPROVED`.

## Activation

None. Tooling, W3. No feature bit, no wire version, no height.

## Test plan

Red tests, `bins/maya2c-anchor/tests/anchor_tests.rs` and
`crates/btc-spv/tests/op_return_tests.rs`:

- `the_payload_is_op_return_m2c1_then_the_blake3_of_the_checkpoint_encoding`
- `a_transaction_with_two_op_returns_is_refused`
- `a_transaction_whose_hash_is_not_the_checkpoints_is_refused`
- `a_proof_under_a_header_off_the_most_work_chain_is_refused`
- `a_proof_shallower_than_min_confirmations_is_refused`
- `a_checkpoint_short_of_quorum_is_refused_before_bitcoin_is_consulted`
- `the_publisher_skips_an_epoch_it_already_anchored` (idempotence,
  against a mocked `bitcoind` RPC)
- `the_publisher_never_signs_without_a_key_file_and_never_logs_it`

A dry-run against Bitcoin **testnet4/signet** with a disclosed address
is the acceptance evidence, written to `reports/ops/anchoring-<date>.md`
with the txids. Mainnet Bitcoin anchoring starts after `APPROVED:
bitcoin anchoring` (it costs money).

## Open questions for Eric (economics only)

1. Who pays the publisher's Bitcoin fees: the foundation operator out of
   pocket, or a treasury reimbursement through ADR-111's spend path
   (which needs a governed cap in native coin and someone to sell it for
   BTC — a process, not a protocol rule).
2. Cadence: one anchor per epoch is the default; fewer anchors cost less
   and widen the undated window.
