# ADR-044: Block rewards and GPU mining at launch, without letting miners order blocks

**Status:** Proposed (2026-10-11), economics decided. Decided by Eric 2026-10-11:
block rewards (no founder premine), GPU mining from block 1. Payout wording
approved 2026-11-01 (DECISIONS.md, "ADR-071 option B and the ADR-044 payout wording").
Zero premine decided 2026-11-07 (DECISIONS.md). Parameters below are defaults
Eric may change until the launch freeze, so the status stays Proposed until he
closes them. Replaces gate 6 of
`docs/mainnet-v1-plan.md` ("fees only", ADR-029) for mainnet.
**Date:** 2026-10-11
**Amended:** 2026-11-24 (Eric): work-proportional pull settlement, share batches, anchor = parent id; see the last section. 2026-11-01 (Eric approved): the payout text then matched `state/mining.rs` (`share_reward`, `epoch_reward`): fixed per-share reward, unpaid budget stays in the reserve, tail floor on `R(e)`. No consensus change.

## Context

Mainnet v1 was fees-only: every coin in the genesis `allocations`, validators
paid from tips, nothing to mine (ADR-029). Eric wants a Bitcoin-style launch:
coins enter circulation by being earned from block 1, by the people running the
network and by anyone with a GPU, and the founder earns the same way, on-chain
and visible, rather than through a premine.

What exists:

- `crypto::dag::hashimoto_light` + `crypto::pow::meets_target` — the DAG proof
  of work (`docs/dag-pow.md`), with conformance vectors.
- `hal/cuda-miner` — CUDA ArgonBlake, asserted bit-identical to the node.
  `bins/maya2c-gpu-miner`, `bins/maya2c-miner`, `crates/stratum-v2`,
  `bins/pool-service`.
- `fee_market::supply::MAX_SUPPLY` = `Amount::MAX` = 2^256 − 1 base units (`crates/fee-market/src/supply.rs`). One coin is 10^24 base units (`maya_amount::ONE_COIN`, `COIN_DECIMALS = 24`). The halving eras issue 19,687,500 coins (ADR-068 corrects the 21,000,000 first written here); there is no cap.
- DAG-BFT finality (ADR-027) with staking epochs of 3,600 blocks.

What must not change: block ordering and finality stay DAG-BFT. A PoW
fork-choice next to BFT finality would reintroduce the reorgs BFT removed and
put two consensus engines on the launch path.

## Decision

**A reward lane, not a second consensus.** Miners never propose, order or
finalize blocks. They earn issuance by publishing work:

1. **`WorkClaim` transaction.** `{ miner: Address, epoch: u64, anchor: Hash,
   nonce: u64 }`. `anchor` is the hash of the last block of epoch `epoch − 1`,
   so work cannot be precomputed before that block is final and cannot be
   replayed into another epoch or chain (the chain tag is in the preimage,
   ADR-036). Work = `hashimoto_light(cache(epoch), H(chain_tag ‖ epoch ‖ anchor ‖ miner), nonce)`
   (the DAG proof of work, `docs/dag-pow.md`) meeting the epoch's share target.
2. **Validity is deterministic and cheap to refuse.** Wrong epoch, wrong
   anchor or a duplicate `(miner, nonce)` is refused before hashing. At most
   `MAX_CLAIMS_PER_BLOCK` claims enter a block, and a claim pays the base fee,
   so verification is bounded per block. A DAG light check costs 2.0 ms on
   a CPU (`docs/dag-pow.md`), against ~25.4 ms for ArgonBlake, which is why
   shares use the DAG. A share pays no base fee, because a new miner holds no
   coin. Spam is bounded by the per-block cap, a per-sender mempool limit, and the
   signature check that runs before the hash.
3. **Emission per epoch, paid from a reserve, never minted.** Genesis
   is funded with an explicit allocation to a keyless `MINING_RESERVE`
   address (`--mining-reserve`, `deploy/genesis/mainnet/README.md`), so the conservation invariant (no native mint, ever) is unchanged.
   Rewards are transfers out of the reserve. `R(e) = max(INITIAL_EPOCH_REWARD >> (e / HALVING_EPOCHS), TAIL_EPOCH_REWARD)`,
   integer only, a function of the epoch alone (never of the reserve balance). With a nonzero tail, genesis refuses a reserve smaller than
   `lifetime_emission()` (an upper bound on everything the schedule pays), so it never runs dry
   (`genesis.rs`, `bft.mining`). Without a tail, emission ends when `R(e)` reaches zero or the reserve is empty. At the epoch's last block:
   - `MINER_SHARE_BPS` of `R(e)` is the epoch's miner budget; each valid claim earns `budget / target_shares` (integer division), capped by what is left of the budget. A claim that lands after the budget is spent still counts and pays nothing;
   - the rest goes to the committee through the existing staking payout (ADR-028);
   - any miner budget left unpaid at the epoch's end, including all of it when no claims land, stays in `MINING_RESERVE`. Nothing is burned and nothing carries into the next epoch's budget; it only extends how long the reserve lasts.
   Total minted is checked against `MAX_SUPPLY` with `check_supply`; emission
   stops at the cap.
4. **Difficulty retargets per epoch** toward `TARGET_CLAIMS_PER_EPOCH`, using
   the integer clamp already in `consensus::difficulty` (max ×4 / ÷4). No
   floats, no wall-clock: it depends only on the previous epoch's claim count.
5. **Genesis carries the parameters**, so a node cannot disagree about them,
   and a genesis without them is fees-only, which keeps every existing testnet
   genesis valid.

### Default parameters (Eric may change before the freeze)

| Parameter | Default | Why |
|---|---|---|
| Emission budget | 19,687,500 coins over the halving phase (corrected 2026-11-07; first stated as 21,000,000) | the protocol cap is `MAX_SUPPLY` = 2^256 − 1 base units |
| `INITIAL_EPOCH_REWARD` | set so that half the cap is issued before the first halving | Bitcoin's shape |
| `HALVING_EPOCHS` | ≈ 4 years of epochs at the *measured* block rate | Bitcoin's interval. Computed from the testnet, not assumed |
| `MINER_SHARE_BPS` | 7000 (70% miners, 30% validators) | mining is the adoption path. Validators still earn fees |
| `TARGET_CLAIMS_PER_EPOCH` | 3,600 (1,000 until the 2026-11-24 amendment) | one share a block on average; verification per block is bounded by the block cap, not by this |
| `MAX_CLAIMS_PER_BLOCK` | 16 | bounds Argon2 work per block |
| Premine | no premine; genesis holds only the keyless mining reserve and the disclosed minimum validator bonds (7 x min-self-bond) | Eric chose rewards only. The bonds are minted value (`zero_supply_mints_only_the_reserve_and_the_bonds`), so they are disclosed rather than called zero |

## Consequences

- Founder coins come from validating and mining from block 1, visible on-chain.
  Early on, with one operator and few miners, most rewards go to Eric. That is
  how Bitcoin's first year looked. It is stated publicly, not hidden.
- Consensus safety is unchanged: a miner can waste block space (bounded and
  paid for), but cannot reorder, stall or finalize anything.
- New consensus-critical code: the apply path, the epoch payout and the
  retarget. It needs conformance vectors, red-team tests (replayed claims,
  wrong-epoch claims, cap overflow, claim floods) and its own testnet soak
  before genesis. **Gate 3 restarts on the build that carries it.**
- The DAG favours GDDR memory bandwidth (4 GiB dataset), so consumer GPUs
  (`hal/cuda-miner` DAG path, `hal/wgpu-miner`) are the intended miners.
  Validators hold only the 64 MiB cache.
- **The lane is built to carry other kinds of work.** `WorkClaim` gets a
  `kind` byte from day one (`0 = DAG PoW`). The intended second kind is
  verified AI inference (Eric, 2026-10-11): a GPU serves a model job, and a
  zkML proof (`crates/zkml`, Halo2) or a challenge game shows it was computed.
  It activates at a written height, after its own ADR. The mainnet format does
  not need to change to allow it.
- Not covered: pools beyond the existing stratum-v2 service; lattice PoUW stays
  RESEARCH (ADR-016, no measured useful output).

## Changes from review (2026-10-11)

Rust review and blockchain security audit of `2e92cbd4`. No value creation or
reserve drain was found. Fixed in `eea9a8af` and `0affbcf5`:

- **Mining opens at the first epoch boundary, not block 1.** Epoch 0's anchor
  is fixed before launch, so its work could be precomputed and land first.
  From epoch 1, the anchor is a block nobody could predict. The committee's
  part of epoch 0 is still paid at the boundary.
- **Mempool admission runs the share rules first**: epoch, spent set, then the
  2 ms proof of work, then the signature. A zero-balance miner's share is
  admitted, and a share that would not pay is refused before it reaches a
  block template.
- **The genesis target is a floor.** Empty epochs can no longer ease the target
  until every digest is a share.
- **Spent shares are keyed by epoch and deleted at the epoch's close.** The
  epoch check already refuses them, so the set no longer grows with the chain.
- **One `validate()` serves genesis and decode.** The reserve must hold at least
  one epoch's reward, and a share that rounds to zero is refused.

Still open:

- **The proposer of an epoch's boundary block learns the next anchor first.**
  It can start on the next epoch's work one block early, and it can censor
  shares it does not include. It cannot steal a share, because the payee is
  bound into the work.
- **Proposer timing under load is unmeasured.** No timing bound on
  `select_applicable` exists yet.

## Changes from the first devnet run (2026-10-12)

The first run of the real node and miner together
(`reports/sessions/2026-10-12-mining-devnet-e2e.md`) paid one epoch and then
stopped. Fixed:

- **A share past the epoch's miner budget is refused.** Before, it applied,
  paid 0, spent the miner's nonce and took a block slot fee-free. The mempool
  runs the same rule, so it now refuses what it said it refused. A share is
  also refused when the reserve can pay nothing.
- **The retarget measures how early the budget ran out.** With overshoot
  refused, `shares` stops near `target_shares` and could only ease the target
  (security review of the fix above). The record now carries `spent_after`,
  the number of blocks into the epoch at which the budget was spent, and the
  retarget uses `shares × epoch_blocks / spent_after` as demand, still clamped
  ×4. The record encoding is version 2; version 1 never reached a public chain.
  Consequence for parameters: a budget can be spent no faster than
  `target_shares / max_shares_per_block` blocks, so the ×4 hardening is
  reachable only if `max_shares_per_block × epoch_blocks ≥ 4 × target_shares`.
- **Miner:** it restarts its transaction nonce at each epoch, because the
  pool evicted the closed epoch's shares and the gap stalled every later
  share. It also idles while the budget is spent and parses `dag` only for
  active work.

## Work credit at launch: stake-only voting (2026-11-02)

Governance voting is stake-only at launch. Eric's decision.

`claim_work` credited each block's genesis-target work to any beneficiary the
claim named, and nothing in the claim proved the sender mined. Voting weight is
locked stake plus decayed work credit, so one fee per block bought weight.

- **Refused, not ignored.** `TxKind::ClaimWork` fails with
  `NodeError::WorkClaimInactive` at every height below
  `WORK_CLAIM_ACTIVATION_HEIGHT`, which is `u64::MAX` at launch. The refusal is
  at apply, so no work record is written and the transaction's fee is not
  charged. Tests: `a_work_claim_is_refused_at_every_height_at_launch_and_credits_nothing`
  and `with_a_refused_claim_present_the_voting_weight_is_locked_stake_alone`
  (`crates/node/tests/governance_tests.rs`).
- **Re-enabled only** when a claim is bound to a verified share in the same
  block and credited to that share's miner. Until then the height stays
  `u64::MAX`. Setting it earlier is a consensus change and needs its own ADR.
- Tests that exercise the credit mechanism itself use
  `BlockContext::with_work_claim_activation(0)` explicitly.
- `docs/audit/KNOWN_ISSUES.md` #2 and #18 say the same.


## Amendment 2026-11-12: `WorkClaim` has no `kind` byte

The "Consequences" bullet that says `WorkClaim` gets a `kind` byte from day one
(`0 = DAG PoW`) does not match the code. `WorkClaim` is
`WorkClaim { beneficiary }` and nothing else
(`crates/node/src/core/governance_payload.rs:120-123`), and its encoding is the
32-byte beneficiary alone (`governance_payload.rs:168-180`). The mainnet
format therefore does need to change before a second kind of work can be
claimed; the sentence "The mainnet format does not need to change to allow it"
is not true as built. The decision above (the lane is meant to carry other
work, activated at a written height after its own ADR) is unchanged. ADR-129
(branch `docs/adr-125-130-gaps`, PR #470) proposes `TxKind::ClaimCompute` as the
carrier instead of a `kind` byte on `WorkClaim`.

## Amendment 2026-11-15: a work-kind byte on both payloads, and a compiled claim guard

Eric 2026-11-14 MCQ ("all format fixes"), before the re-genesis. This replaces
the 2026-11-12 amendment's statement of fact: the byte now exists.

- **`WorkKind`, a closed `#[repr(u8)]` enum** (`crates/node/src/core/mining_payload.rs`).
  `0 = DagPow` is the only variant. Both payloads begin with it:
  `MiningShare` is `kind ‖ epoch (u64 LE) ‖ nonce (u64 LE)`, 17 bytes, and
  `WorkClaim` is `kind ‖ beneficiary[32]`, 33 bytes. Any other byte is a decode
  error (spec MINE-11), so a frame naming an unknown kind is refused at
  `Transaction::from_bytes`, before RPC, gossip, the mempool or block import
  see it. Same reasoning as `SuiteId` (invariant 29): a kind some nodes know
  and others do not is a fork, so there is no fallback.
- **Extending the registry** is a code change, not a vote: a new variant in a
  new binary, refused by the executor below the activation height of its own
  `FeatureSet` bit, which only a scheduled upgrade sets (ADR-082). ADR-079's
  `kind = 1` (verified compute) and ADR-129's `TxKind::ClaimCompute` remain
  alternatives; this byte makes the first possible without another format
  change, and decides neither. The share header (MINE-9) and the spent-share
  key (`seen_key(epoch, header, nonce)`) do not commit to the kind, so a new
  kind must use its own header domain and its own spent-key prefix, or the
  same nonce could be paid once per kind. Both executors
  (`apply_mining_share`, `claim_work`) destructure the kind irrefutably, so
  adding a variant is a compile error at the place its gate belongs.
- **`WORK_CLAIM_ADR_ACCEPTED = false`** (`crates/node/src/upgrade.rs`). While it
  is false, `FeatureSet::WORK_CLAIM` (bit 4) is outside
  `FeatureSet::IMPLEMENTED`, the same model as zkML under the ADR-082
  amendment of 2026-11-12 (PR #506). A genesis schedule naming it is refused at
  boot (`GenesisConfig::upgrade_schedule`). A governance entry naming it is
  admitted by every binary, written, and halts this binary with
  `UpgradeRequired` at its activation height, and `features().work_claim`
  stays `u64::MAX`. Refusing the entry at the vote instead would split this
  binary from a later one that has the ADR, at the block carrying the vote
  (review of PR #570). `Mempool::validate` refuses `TxKind::ClaimWork` with
  `NodeError::WorkClaimInactive`, and the executor's refusal (above) is
  unchanged. The flag is set to `true` only in the change that lands an
  Accepted ADR binding a claim to a verified share.

Tests: `crates/node/tests/payload_tests.rs`
(`a_share_and_a_claim_encode_kind_zero_first`,
`every_work_kind_byte_but_zero_is_refused_at_decode`,
`an_undefined_work_kind_is_refused_at_mempool_admission`),
`crates/node/tests/upgrade_schedule_tests.rs`
(`work_claim_halts_this_binary_at_activation_until_its_adr_is_accepted`),
`crates/node/tests/governance_tests.rs`
(`a_work_claim_is_refused_at_mempool_admission`); vectors `spec/tests/mining.json`
(`MINE-11`), replayed by `crates/node/tests/conformance.rs` and
`spec/verifier-ts`. The share path is otherwise unchanged:
`crates/node/tests/mining_rewards_tests.rs` runs as before.

This amendment depends on PR #506 (unknown or unimplemented upgrade bits are
admitted and halt at activation instead of being refused): the branch merges
#506, so #506 merges first.

## Amendment 2026-11-24: work-proportional pay for any number of miners

Eric 2026-11-24 (DECISIONS.md, "Mining open to any miner, solo to hundreds"):
"make sure even a sole miner can mine, or hundreds of miners; maximum
flexibility for any kind of miner". Items MINE-01, MINE-03, M10 and issue
#642, all chosen as recommended. Consensus and state format change; it ships
with the re-genesis. This replaces the per-share payout of the 2026-11-01
amendment and the "fixed per-share reward" text above.

**Guarantees.**

1. **Pay is proportional to work, never to arrival order.** A share is
   credited to its miner, not paid. When the epoch closes, its miner budget is
   split by work: a miner with `m` of the epoch's `n` shares earns
   `⌊budget × m / n⌋` (spec MINE-4). A share that lands in the epoch's last
   block earns exactly what one in its first block does, so a solo miner on a
   slow link, far from the validators, is not out-raced by a pool next to
   them. Before, the first `target_shares` shares took the whole budget and
   later ones were refused (audit MINE-01).
2. **Closing an epoch costs the same for one miner as for any number.** The
   boundary writes one record, the epoch's pot `k:mpot:<epoch>` (budget, shares,
   uncollected credits), and reads or writes no miner's record. Each miner
   collects with its next share (the next epoch's first share pays the closed
   one) or with a claim. A miner holds one credit record, `k:mcredit:<miner>`,
   whatever the number of epochs it mines; a pot is deleted with its last
   credit (audit M10).
3. **Many shares, one signature.** `MiningShare` is now a batch:
   `kind ‖ epoch (u64 LE) ‖ count (u32 LE) ‖ nonces (u64 LE, strictly
   ascending)` (spec MINE-11). One post-quantum signature covers every nonce,
   so a share past the first costs 8 bytes on chain instead of a full signed
   transaction of about 13 KB (audit MINE-03). Ascending order gives a batch
   one encoding and rules out a repeated nonce at decode. The batch counts one
   block slot per nonce against `max_shares_per_block`, and one bad nonce fails
   the whole transaction. A batch with no nonces is the claim: it names a
   closed epoch the sender holds a credit for. Shares and claims stay fee-free
   when they move nothing (FEE-7), and the mempool's one-entry-per-work rule
   (#653) applies to every nonce of a batch.
4. **The anchor is the boundary block's parent** (issue #642). The next
   epoch's anchor is `header.prev_hash`, not `header.id()`. The id covers the
   state root, and the anchor is part of the state that root commits to, so a
   validator that derived the block (root zeroed, preview reused) and a node
   replaying it fresh computed different anchors and different roots at every
   mining boundary. The parent is final and identical on every node, and it is
   still unknown before the epoch's last block, so work still cannot start
   before the epoch is nearly open (spec MINE-12).

**What else changes.** Record version 5 drops `paid` and `spent_after` and adds
`miners`. With no budget refusal, `shares` is the epoch's demand, so the
retarget reads it directly (MINE-5); the `spent_after` scaling of the
2026-10-12 review is gone. An epoch with shares now pays its whole miner
budget; an epoch without shares leaves it in the reserve, as before, so the
19,687,500-coin figure stays an upper bound. `share_reward` survives as an
estimate for RPC and miners, `budget / target_shares`, never a consensus
value. `deploy/genesis/mainnet/mining.json` raises `target_shares` from 1,000
to 3,600, one share per block on average at one-hour epochs: a solo miner
finds a paid share 3.6 times as often for the same hashrate, and the block cap
of 16 still leaves 16 times the target in slots, four times the retarget's ×4
clamp (`16 × 3,600 ≥ 4 × 3,600`). The per-block DAG verification bound is
unchanged, since it is set by `max_shares_per_block`, not by the target.

**Arithmetic, not a measurement.** At the target, an epoch carries 3,600 shares.
Sent one per transaction that is 3,600 signed transactions; sent in batches of
`b` it is `⌈3,600 / b⌉` signatures plus 8 bytes a share. No byte or verify
figure here was measured on a running network.

**Not done here.** `--claim` in `maya2c-gpu-miner` (a miner that stops mining
collects with a claim transaction, which the miner does not build yet); an RPC
method for a miner's credit (the state accessors `committed_credit` and
`committed_pot` exist); the `16E ≥ 4T` check at genesis (ceiling row M7).

Tests: `crates/node/tests/mining_rewards_tests.rs`
(`a_solo_miner_and_a_hundred_miners_are_each_paid_in_proportion_to_work`,
`a_fresh_replay_reproduces_every_root_across_mining_boundaries`,
`an_epoch_is_anchored_to_its_boundary_blocks_parent`,
`an_extra_share_in_a_batch_costs_eight_bytes`,
`a_batch_counts_every_share_against_the_block_cap_and_fails_whole`,
`a_miner_that_keeps_mining_is_paid_by_its_next_epochs_first_share`,
`a_share_is_credited_then_its_claim_is_paid_from_the_reserve_with_no_fee`,
`shares_past_target_still_land_split_the_budget_and_harden_the_target`),
`crates/node/tests/payload_tests.rs`
(`a_share_batch_round_trips_and_refuses_repeats_disorder_and_short_counts`),
`crates/node/src/state/mining.rs`
(`a_payout_is_the_floor_of_budget_times_work_over_total`); vectors
`spec/tests/mining.json` (MINE-4, MINE-5, MINE-6, MINE-11, MINE-12), replayed by
`crates/node/tests/conformance.rs` and `spec/verifier-ts`.
