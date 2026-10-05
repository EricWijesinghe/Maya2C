# Session 2026-10-04/05: attacknet, rejoin fixes, testnet.2

## What was done
- **Site** (#81): full-bleed hero, phone layout, and light mode with no
  dark boxes.
  - Audit command: `node audit.mjs` in the scratch audit dir, 19 pages × 3
    widths × 2 themes.
  - Result against the live site: no overflow, no clipped text, no dark
    boxes.
- **Stake-weighted genesis** (#77). Review HIGH fixed: keys and weights now
  come from one boundary snapshot. Regression test
  `a_weighted_node_restarts_after_a_mid_epoch_tombstone_and_keeps_weights`
  fails without the fix.
- **`cargo xtask attacknet`** (#85): 7 validator processes, 6 attacks per
  round, and a no-fork check at every height. Bugs it found, all fixed:
  1. a bootstrap aborted on one HTTP 429;
  2. an f+1 restart left validators as permanent followers;
  3. no rejoin across an epoch boundary;
  4. late attestations were dropped at an epoch switch;
  5. a follower never switched epochs;
  6. one far-future attestation wiped every pending checkpoint;
  7. bootnodes were never redialled;
  8. validators one block behind became followers, which starved the
     checkpoint quorum.
- **Decision (Eric):** checkpoints keep n − f, a >2/3 trust floor. An f + 1
  variant fixed the stall but lowered that floor to >1/3, so it was
  reverted (ADR-038).
- **Ops:**
  - testnet.2 tagged.
  - Seed, gateways and status redeployed from master 9917db56.
  - `node-config.toml` raises the node RPC limit from 50/s to 1000/s. The
    gateways logged `429` from the node under peer load.
  - 12 project peers running.

## Evidence (real output)
- `cargo xtask attacknet --rounds 3`, five consecutive runs:
  - run 1: no fork through height 226
  - run 2: no fork through height 221
  - run 3: no fork through height 224
  - run 4: no fork through height 233
  - run 5: no fork through height 251
- `cargo nextest run -p custom-l1-node --test bft_node_tests --test bft_staking_tests --lib`:
  376 passed, 1 skipped.
- `cargo nextest run -p maya-api-gateway`: 57 passed.
- PR #85 CI: 27 pass, 1 skipping.
- Peers: 11 of 12 level with the seed (89,786–89,787 against 89,787).

## Not verified
- The testnet.2 release assets: the run was still queued.
- `join-testnet.sh` end to end against testnet.2: the WSL dry run passed
  the release and checksum steps on testnet.1, and the genesis step
  failed. That 404 is fixed by #83.

## Honest limits
- The attacknet is one operator on one machine over loopback. It is not
  an independent test, an incentivised testnet or an audit.
- Project peers are one operator on one IP. They do not count toward
  gate 4.
