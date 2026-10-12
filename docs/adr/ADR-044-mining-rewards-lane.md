# ADR-044: Block rewards and GPU mining at launch, without letting miners order blocks

**Status:** Proposed (2026-10-11). Economics decided by Eric 2026-10-11:
block rewards (no founder premine), GPU mining from block 1. Parameters below
are defaults Eric may change until the launch freeze. Replaces gate 6 of
`docs/mainnet-v1-plan.md` ("fees only", ADR-029) for mainnet.
**Date:** 2026-10-11

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
- `fee_market::supply::MAX_SUPPLY` = 21,000,000 coins × 10^8.
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
   allocates `MAX_SUPPLY − Σ allocations` to a keyless `MINING_RESERVE`
   address, so the conservation invariant (no native mint, ever) is unchanged.
   Rewards are transfers out of the reserve, and emission ends when it is empty. `R(e) = INITIAL_EPOCH_REWARD >> (e / HALVING_EPOCHS)`,
   integer only. At the epoch's last block:
   - `MINER_SHARE_BPS` of `R(e)` is split across the epoch's valid claims,
     pro rata by claim count (each claim is one share at the same target);
     integer division, remainder burned;
   - the rest goes to the committee through the existing staking payout
     (ADR-028);
   - an epoch with no claims burns the miner share instead of carrying it.
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
| Cap | 21,000,000 | already `MAX_SUPPLY` |
| `INITIAL_EPOCH_REWARD` | set so that half the cap is issued before the first halving | Bitcoin's shape |
| `HALVING_EPOCHS` | ≈ 4 years of epochs at the *measured* block rate | Bitcoin's interval. Computed from the testnet, not assumed |
| `MINER_SHARE_BPS` | 7000 (70% miners, 30% validators) | mining is the adoption path. Validators still earn fees |
| `TARGET_CLAIMS_PER_EPOCH` | 1,000 | small enough to bound verification |
| `MAX_CLAIMS_PER_BLOCK` | 16 | bounds Argon2 work per block |
| Premine | none | Eric chose rewards only |

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
