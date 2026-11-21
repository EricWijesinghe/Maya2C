# ADR-121: Blockspace reservation — lanes as a reserved slice of every author's quantum

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-24, [prior-art/blockspace-reservation.md](../prior-art/blockspace-reservation.md), [ADR-072](ADR-072-max-block-bytes.md), [ADR-029](ADR-029-fee-market-live.md), [ADR-076](ADR-076-base-fee-step-clamp-and-ceiling.md), [ADR-027](ADR-027-dag-bft-in-the-node.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 17, 24, 25, 28, 33; Standing Order 4

## Context

`crates/lanes` (Master Prompt 26) models reserved capacity: a pay-as-bid
auction, highest price first, at most `per_holder` slots each
(`lib.rs:144-164`), and a builder that fills lane transactions first and
releases unused slots to the general pool in the same block
(`lib.rs:182-211`). It is integer-only and "not wired into the node"
(`lib.rs:18-19`; `features.toml:3297`).

**Finding that contradicts the brief, and the crate.** `lanes::build`
assumes one builder choosing from one pool. The node has no such builder.
Under DAG-BFT every validator packs its *own* vertex from its *own* queue,
FIFO (`crates/dag-bft/src/validator.rs:97-114` `take_batch`), ranked at
admission by fee per byte (`network/mempool.rs:187-193`); the block is a
pure function of the committed sub-DAG (`commit.rs:11-12`), and the
transactions admitted are cut by a deficit round-robin over authors with a
quantum `DECODE_BUDGET / n` fixed at genesis (ADR-072 CON-13,
`block.rs decode_plan`, `builder.rs:62-99`). So "a share of the block" is
not a thing one party can sell: the unit of capacity is **an author's
quantum**, and `n` authors each decide what fills theirs. A lane that only
one validator honours is worth `1/n` of what it promises. The reservation
must be a consensus rule applied inside the decode walk, which is where this
ADR puts it, and `lanes::build` is not the function that gets wired.

Ethereum's execution tickets and ETHGas's preconfirmation market sell a
proposer's future block; Solana has no reservation primitive and prices
contended accounts locally (prior-art file; `docs/prior-art/local-fee-markets.md`).

## Decision

**A lane is a committed record: a holder, a term in epochs, and
`bytes_per_block`. In the decode walk, before the fair round-robin, every
author's quantum admits lane-tagged payloads from that author's queue up to
each lane's `bytes_per_block / n` (ceiling), charged to the lane, not to the
author's deficit; the rest of the walk is unchanged. Lanes are sold by the
`crates/lanes` auction at each epoch boundary for a future epoch, pay-as-bid,
with a compiled share ceiling so no holder and no set of holders can buy the
chain.**

### Why `bytes_per_block / n` per author

A lane holder's transaction reaches whichever validators' mempools gossip
delivers it to; each honest author that holds it packs it in its next
vertex (a lane transaction sorts first in `take_batch`'s queue, client
rule). In the walk every author's reserved slice is `ceil(bytes_per_block /
n)`, so across `n` authors the lane gets at most `bytes_per_block + n` and,
if only one honest author carried it, at least `bytes_per_block / n`. That
floor is stated to holders as the guarantee under `f` faulty authors:
`(n − f) / n` of the reservation, the same honest-share bound ADR-072
gives everyone.

### State (invariant 25)

Prefix `ln:` in a new layer `StateLayer::Lanes`, in `RECORD_LAYERS` and the
undo journal:

- `ln:l:` ‖ `epoch(8 BE)` ‖ `lane_id(32)` → `Lane { holder: Address,
  bytes_per_block: u64, price_per_block: U256 }`; `lane_id =
  blake3::derive_key("maya2c lane v1", holder ‖ epoch ‖ index)`.
- `ln:b:` ‖ `epoch(8 BE)` ‖ `bidder` ‖ `index(2 BE)` → `Bid { bytes_per_block,
  price_per_block }`, escrowed in full (`price_per_block × EPOCH_LENGTH`) at
  bid time, refunded to losers at the boundary.
- `ln:u:` ‖ `lane_id` → bytes used this epoch (statistics, for the explorer).

### Tagging a transaction

`TxKind::WithLane { lane_id: [u8; 32], inner: Box<TxKind> }`, one level, in
the v6 payload frame (ADR-112's wrapper shape; no wire version). The
**signer** must be the lane's holder, or an address the holder listed in a
`LaneMembers { lane_id, members: Vec<Address> }` record (`ln:m:`, at most
`MAX_LANE_MEMBERS = 64`), so an app can reserve space for its users. A
`WithLane` whose lane is absent, expired or not the signer's is admitted as
`inner` with no reservation (invariant 7: not a refusal, because the
transaction is otherwise valid and the holder's mistake should not lose it
its place in the general pool).

### Auction at the epoch boundary (consensus step)

At the last block of epoch `e`, after ADR-109/ADR-114's passes: read `ln:b:`
for epoch `e + LANE_LEAD_EPOCHS` (2: a *future* epoch, which is what makes a
lane a forward reservation), run `lanes::auction` with `total =
LaneTotalShareBps × target_block_bytes / 10_000` and `per_holder =
LaneMaxHolderShareBps × target_block_bytes / 10_000`, both governed
(invariant 17) under compiled ceilings `MAX_LANE_TOTAL_BPS = 5_000` and
`MAX_LANE_HOLDER_BPS = 1_000` (`crates/governance/src/limits.rs`); write the
winning `ln:l:` records, refund losers, and move winners' escrow to
`FEE_COLLECTOR` (joins the tips, `fees.rs:14-17`; question 1). Bids are
visited in key order; `auction` sorts by `(price desc, holder)` already
(`lib.rs:145`), which is deterministic. At most `MAX_LANE_BIDS_PER_EPOCH =
1_024` bids are admitted.

Reserved space a lane does not use is general space in the same walk
(`lib.rs:12-13`'s rule kept): the lane slice is a floor for its holder, not
a hole in the block.

### Fees

A lane transaction still pays the base fee per byte (it burns; ADR-029) and
needs no tip. The lane price is what the holder paid for *certainty of
inclusion up to the slice*, not a discount. `target_block_bytes` counts
lane bytes like any other, so a full lane raises the base fee as any demand
does (ADR-076's controller is untouched).

## Alternatives considered

- **Wire `lanes::build` as written.** Requires a single builder; the chain
  has `n`. Rejected on the finding above.
- **Sell whole quanta (execution tickets).** Sells an author's seat, which
  is a validator's stake-weighted right, not a user's. Rejected.
- **Per-validator private deals (preconfirmations).** Off-chain, unenforced
  by consensus, and ADR-025 forbids validators selling ordering. Rejected.
- **Lanes as a fee discount.** Then a lane is a subsidy bought at auction;
  the base fee must burn the same for everyone (invariant 17's reason).
- **Current-epoch auction.** Not a reservation; `LANE_LEAD_EPOCHS = 2` is
  the "future" in "lane futures", and bounded so escrow is short.

## Consequences

- Apps can buy guaranteed throughput two epochs ahead, bounded to half the
  target and a tenth per holder; the floor under faults is stated.
- `decode_plan` gains a reserved pass; `builder.rs` tests gain lane cases;
  one wrapper kind, two records kinds, one boundary step, one layer.
- `crates/lanes::auction` is reused as-is; `lanes::build` stays a model
  and its doc says so.
- Block work: one extra pass over each author's prefix, bounded by the
  decode budget already in force.

## Invariants touched

- **17**: share parameters are governed under compiled ceilings.
- **24**: the reserved pass is in `decode_plan`, which every validator runs
  (ADR-072's "end-of-block drops are consensus" rule covers it).
- **25**: `ln:` prefix and layer; undo covers bids, lanes, members.
- **7**: an invalid lane tag degrades to `inner`, never refuses.
- **28**: `Module::Lanes` gates bids and members; a breaker makes every
  `WithLane` plain `inner`.
- **33**: lane bytes are inside the same cap; nothing grows.
- Standing Order 4: key-order bids, deterministic sort, no clock.

## Activation

`FeatureSet::LANES = 1 << 28` (claimed in `adr-claims.txt`), compiled
`LANES_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with the v4
bundle at **genesis + 15,552,000**. Before activation bids and members are
refused and `WithLane` is admitted as `inner`.

## Test plan

Red tests, `crates/node/tests/lane_tests.rs` and `consensus/bft/builder.rs`:

- `a_lane_payload_is_admitted_ahead_of_the_fair_walk_up_to_its_slice`
- `a_lane_slice_unused_is_general_space_in_the_same_block`
- `a_junk_heavy_author_cannot_starve_a_lane_carried_by_another_author`
- `one_honest_author_carrying_the_lane_gives_at_least_one_nth`
- `a_non_member_signer_with_a_lane_tag_is_admitted_as_inner`
- `the_auction_at_the_boundary_is_pay_as_bid_and_refunds_losers`
- `no_holder_exceeds_the_holder_share_and_the_total_is_capped`
- `a_lane_transaction_still_burns_the_base_fee`
- `a_reorg_restores_bids_lanes_and_escrow`

Vectors (`spec/03-state.md` LN-1..LN-3): a bid, a lane, a boundary result.
`crates/lanes/tests/lanes_tests.rs` gains the `/ n` ceiling arithmetic.

## Open questions for Eric (economics only)

1. Where auction proceeds go: the tips pool (as written), burn, or the
   treasury (ADR-111).
2. `LaneTotalShareBps` and `LaneMaxHolderShareBps` at activation, under the
   compiled 50% / 10% ceilings.
3. A reserve price per byte (a floor bid), or let the auction clear at any
   price above zero?
