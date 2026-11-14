# ADR-119: A data-carrier kind and the indexer convention for metaprotocols

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-22, [prior-art/data-carrier.md](../prior-art/data-carrier.md), [ADR-072](ADR-072-max-block-bytes.md), [ADR-029](ADR-029-fee-market-live.md), ADR-092 (public indexer API, PR pending), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 28, 31, 33

## Context

No transaction kind carries free bytes. `TxKind` (`crates/node/src/core/payload.rs:438-554`)
is a closed list of typed payloads; the closest shapes are
`AttestLegal` ("a hash, never a document") and `AnchorAttestation`. The
plain-transfer frame has no memo (`transaction.rs:173-196`). A contract call
carries `input` bytes, but only to a deployed contract, and a module costs
`VmMaxModuleBytes` to deploy. People who want to write on this chain today
would do what Bitcoin users did before Core 30: hide bytes in fields meant for
something else (prior-art file), which the indexer cannot tell from noise.

The byte economics already exist: every transaction pays a base fee per
serialized byte that burns (`state/fees.rs:1-17`), the block is capped at
`MAX_BLOCK_BYTES` = 5 MiB with first-fit selection (invariant 33,
`block.rs:41-52`), and the base fee steps toward `target_block_bytes`
(`fee-market`, `MIN_TARGET_BLOCK_BYTES` = 64 KiB, `MAX_TARGET_BLOCK_BYTES` =
2.5 MiB, `limits.rs:42-49`). A data flood therefore raises the price of
every byte, which is the designed response, not a failure.

**Finding on the brief.** The row says "wire-format change". It is not one:
a new `TxKind` arm rides the existing v6 payload frame
(`transaction.rs:118`, `WIRE_VERSION_PAYLOAD`), as ADR-112's `WithReports`
does. No wire version is claimed.

## Decision

**A `TxKind::Data { tag: [u8; 4], body: Vec<u8> }` kind that writes nothing
to state. The tag names a metaprotocol; the body is opaque to consensus.
Indexers index by tag. The body is capped at `MAX_DATA_BYTES = 64 KiB` so one
data transaction cannot take a block's target, and the ordinary byte fee is
the only price.**

### Consensus rules

- `body.len() <= MAX_DATA_BYTES` (64 KiB), checked at decode with
  `read_byte_len`, like `MAX_SEALED_CIPHERTEXT` (`sealed_payload.rs:34`).
- The transaction must still carry its fee output and verify like any other;
  `inputs` and `outputs` are ordinary (a data transaction can also pay
  someone, which is how a metaprotocol transfers something it defines).
- Execution writes **no** record. `Module::of(Data)` is `None`, like
  `Transfer` (`invariant_guard/mod.rs`), because there is nothing a breaker
  could protect; it is still a `Some`-less match arm, not a wildcard.
- A `Data` transaction is in the stateless-verifiable set
  (`stateless.rs:14-31`): it stages nothing, so a witness decides it.
- Not a vault door: it moves no value by itself; outputs count as a plain
  transfer does (invariant 32).

### Indexer convention (not consensus)

`docs/metaprotocols.md` (written with the implementation) fixes:

- `tag` is four ASCII bytes, registered in that file by PR; unregistered tags
  are indexed as `unknown`. Registration is a documentation act, never a
  chain rule: two apps using one tag are an app problem, as on Bitcoin.
- `body[0]` is the metaprotocol's own version byte by convention.
- The indexer (ADR-092 API) exposes `data_by_tag(tag, from_height, limit)`
  and `data_by_txid(txid)`, returning `(height, txid, sender, body)` in
  block order. Sender is the transaction's signer address (one of
  `public_key` / `suite_auth` / `multisig` per the frame), so a metaprotocol
  has an authenticated author without defining signatures of its own.
- Archive nodes keep the bytes as block bodies (invariant 27); a pruned node
  does not serve them; the indexer serves them from its own store.

### What a metaprotocol gets and does not get

It gets ordered, signed, fee-paid, archived bytes with an author. It does
not get state, validation of its own rules, or any promise that a node
checked anything but the length. The docs say so in those words.

## Alternatives considered

- **A memo field on the transfer frame.** Changes every wire version and
  every signer (invariant 31) for a feature a kind provides. Rejected.
- **Unlimited body (Bitcoin Core 30's OP_RETURN).** The block cap already
  bounds it at `MAX_TX_BYTES` = 4 MiB, but one such transaction is 80% of a
  block and starves the fair-share walk's quantum (ADR-072 CON-13). 64 KiB
  keeps a data transaction inside any committee's quantum at `n <= 120`.
- **A separate data fee lane.** ADR-123's blobs are that: bytes that expire
  and never enter the block. Permanent bytes pay the permanent-byte price.
- **A state record per data transaction (`AnchorAttestation` style).** Then
  the chain stores it twice. Nothing is written.
- **Refuse the kind, keep people hiding bytes in `input`.** The indexer then
  has nothing to key on and the nodes store the bytes anyway.

## Consequences

- One `TxKind` arm, one length constant, one `describe.rs` line, one
  `Module::of` arm, one stateless-set entry; no state, no layer, no root
  movement.
- Metaprotocols can exist and be indexed; their rules are theirs.
- Under a data flood the base fee rises for everyone, as ADR-029 designed;
  `target_block_bytes` is the governed lever (ADR-076).

## Invariants touched

- **31**: no frame changes; the kind rides v6 and is signed as any payload.
- **33**: bounded body; first-fit unchanged.
- **7**: an over-length body is a decode refusal.
- **28**: `Module::of(Data)` is `None`, by an explicit arm.

## Activation

`FeatureSet::DATA_CARRIER = 1 << 26` (claimed in `adr-claims.txt`),
compiled `DATA_CARRIER_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082
with the v4 bundle at **genesis + 15,552,000**. Before activation `Data` is
refused at admission and in `apply_block_journaled`.

## Test plan

Red tests, `crates/node/tests/data_carrier_tests.rs`:

- `a_data_transaction_moves_no_root_but_its_fee_burn`
- `a_body_one_byte_over_the_cap_is_refused_at_decode`
- `a_data_transaction_with_outputs_pays_them_like_a_transfer`
- `a_block_of_data_transactions_raises_the_base_fee` (ADR-029 step)
- `a_data_transaction_is_stateless_verifiable`
- `before_activation_data_is_refused`
- `the_indexer_returns_data_by_tag_in_block_order` (`crates/indexer`)

Vectors (`spec/02-transactions.md` DC-1): one `Data` payload and its txid.
`cargo xtask spec-coverage` lists it.

## Open questions for Eric (economics only)

1. Should permanent data bytes pay a **multiplier** on the base fee (as
   Bitcoin weights witness bytes at a quarter, in the other direction), or
   the same per-byte price as any transaction? Default: the same price; the
   target-bytes controller is the lever.
2. `MAX_DATA_BYTES` (64 KiB) compiled, or governed under a compiled ceiling?
