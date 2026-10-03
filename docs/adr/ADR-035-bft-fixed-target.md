# ADR-035: DAG-BFT blocks keep the genesis difficulty target

**Status:** Accepted (2026-09-29). A block-validity rule for DAG-BFT chains.
It supersedes the `difficulty_target` row of ADR-027's field table.
**Date:** 2026-09-29

## Context

maya-testnet-1, the first public DAG-BFT network, halted at height 12,530,
about three and a half hours after genesis. From then on, every block its
validator built was filed as a side branch:
`a locally built DAG-BFT block did not extend the tip: SideBranch`,
13,333 times before anyone looked.

ADR-027 kept the proof-of-work header layout and gave `difficulty_target`
this meaning under DAG-BFT: "the retarget rule at the unlimited floor; no
work is verified." The retarget was treated as harmless decoration. It was
not:

1. DAG-BFT produces a block about every second. The retarget aims for
   15-second spacing, so each 100-block window came in far too fast, and the
   target was hardened by the maximum factor every window. On the testnet it
   went from `ffff…` at block 1 to `0000…0fff…` by 3,000 and `…003f` by
   12,529.
2. A block's work is `2^256 ÷ (target + 1)`, so it roughly quadrupled per
   window.
3. `Chain::insert_block` sums work with a saturating add. The sum reached
   2^256 − 1, and a new block's total can then never exceed the tip's.
   `total_work <= self.total_work()` files it as a side branch, and the
   chain can never extend again.

The spec already said CON-5 (the retarget) applies to proof-of-work chains
only, but it never said what the target *is* under DAG-BFT, and the code
kept retargeting. Every DAG-BFT network would halt at about the same height.
The devnet runs that proved DAG-BFT (60 seconds, about 60 blocks) never came
close.

## Decision

Under DAG-BFT, **a block's difficulty target equals its parent's** (spec
CON-9). Every block carries the genesis target: there is no retarget and no
DAG activation pin.

- `ChainConfig::dag_bft()` sets a new `fixed_target` flag, and
  `Chain::next_target` then returns `dag_bft_target(parent)`. The node uses
  this config for any genesis with a `bft` section.
- Each block adds the same work, so total work is linear in height. At the
  unlimited target it is exactly `height + 1`, and it cannot saturate.
- **A DAG-BFT genesis must set `difficulty_bits` and `pow_limit_bits` to 0**
  (`GenesisConfig::validate`). With the target fixed, each block adds
  `2^difficulty_bits` of work. Nothing is verified, so a hard target buys
  nothing, while at 250 bits total work would saturate within a handful of
  blocks. Found by the security review.
- `maya2c fork`/`replay` (`bins/maya2c-cli/src/fork.rs`) and the `bft_tps`
  benchmark open DAG-BFT chains with the same config as the node. The Rust
  review found fork/replay still on the retargeting config, which would have
  refused the node's own blocks on replay.
- Fork choice is untouched. The fix is to stop the quantity it compares from
  running away, not to special-case the comparison.

Rejected: making `insert_block` extend a DAG-BFT tip regardless of work.
That keeps a meaningless, saturated number in the block records and changes
the invariant-24 apply path for a problem the target rule causes.

## Consequences

- **The existing chain cannot continue.** Its blocks after the first retarget
  window carry targets the new rule refuses. maya-testnet-1 is restarted
  from a new genesis. It had been public for minutes, no release was tagged,
  and the site already says the testnet may be reset.
- `crates/node/tests/bft_work_saturation_tests.rs` builds 13,000 one-second
  blocks under the new rule and requires every one to extend the tip (2.7 s).
  The same test under the old config fails at block 12,531. It also keeps a
  test demonstrating that the old retarget hardens a one-second chain, so the
  mechanism stays shown, not just remembered.
- Spec CON-9 has conformance vectors from the reference implementation
  (`spec-ref`), checked against the node by `consensus_conformance`.
- Proof-of-work chains are unchanged; CON-5 to CON-7 still apply to them.
- **Operator procedure for a chain that halted this way:** start the new
  binary on a **new data directory with a new genesis**. On the old data
  directory it cannot resume: the tip's total work is already saturated, so
  every block is still a side branch. Fund the new genesis with **fresh
  keys**: transaction signatures do not commit to the chain yet (BACKLOG.md,
  P1), so transfers signed on the old chain would replay on a new one funded
  by the same keys.
- Lesson: a long-running property ("the chain keeps extending") needs a test
  that runs long enough to break it. 60-second devnets prove liveness for 60
  seconds.
