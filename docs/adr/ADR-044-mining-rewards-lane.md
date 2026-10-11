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

- `crypto::argon_blake::argon_blake_hash` + `crypto::pow::meets_target` — the
  chain's original proof of work, still used by devnet mode (ADR-015).
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
   ADR-036). Work = `argon_blake_hash(chain_tag ‖ epoch ‖ anchor ‖ miner ‖ nonce)`
   meeting the epoch's share target.
2. **Validity is deterministic and cheap to refuse.** Wrong epoch, wrong
   anchor or a duplicate `(miner, nonce)` is refused before hashing. At most
   `MAX_CLAIMS_PER_BLOCK` claims enter a block, and a claim pays the base fee,
   so Argon2 verification (32 MiB, the costly step) is bounded per block.
3. **Emission per epoch.** `R(e) = INITIAL_EPOCH_REWARD >> (e / HALVING_EPOCHS)`,
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
- ArgonBlake favours memory bandwidth (32 MiB per hash), so consumer GPUs stay
  competitive with each other. A CPU miner works, but slower.
- **The lane is built to carry other kinds of work.** `WorkClaim` gets a
  `kind` byte from day one (`0 = ArgonBlake`). The intended second kind is
  verified AI inference (Eric, 2026-10-11): a GPU serves a model job, and a
  zkML proof (`crates/zkml`, Halo2) or a challenge game shows it was computed.
  It activates at a written height, after its own ADR. The mainnet format does
  not need to change to allow it.
- Not covered: pools beyond the existing stratum-v2 service; lattice PoUW stays
  RESEARCH (ADR-016, no measured useful output).
