# ADR-043: A way back after any outage, not only a short one

**Status:** Proposed (2026-10-10). Operational half applied on maya-testnet-1;
code half open. Mainnet launch gates 8 and 10 (`docs/mainnet-v1-plan.md`).
**Date:** 2026-10-10

## Context

ADR-038 lets a validator rejoin through quorum-attested checkpoints. On
2026-10-10 the live testnet showed where that path ends:

- The seed validator (v1) was killed at height ~340,966 on 2026-10-09 23:34 by
  a lab script that stopped processes by image name, and the watchdog, also
  matching by name, never restarted it. The chain went on with v2..v4 (n − f =
  3 of 4, so zero tolerance left) to ~388,400: 13 epochs.
- `--catch-up-from` failed: `the peer holds no checkpoint of epoch 94, which it
  keeps for 8 epochs`.
- `--bootstrap-from` on a fresh data directory failed: `this node serves no
  snapshots`. Only the seed served snapshots, and the seed was the node that
  was behind.
- A validator restarted with `--catch-up-from <seed>` while the seed was down
  would also have failed startup, because the catch-up source was fixed.

So a node is recoverable only while (a) it is fewer than 8 epochs behind, or
(b) some *other* live node serves a snapshot at least `--prune-depth` deep.
Neither held. Nothing about it is specific to the seed: any validator down
for a long holiday, a dead disk, or a new operator joining late meets it.

## Decision

1. **Every validator serves snapshots** (`--snapshot-interval 3600
   --snapshot-depth 3600`). Applied to v2..v4 on 2026-10-10. With n ≥ 2
   serving, the node that is behind is never the only source.
2. **The catch-up source is chosen at start, not fixed**: the live node with
   the highest tip other than the node itself; none live (cold boot of the
   whole set, so all stopped at the same round) → start without catch-up,
   which is an ordinary in-window restart. Applied in
   `D:\Maya2C-validators\start-validators.ps1`.
3. **The documented way back** for a node more than 8 epochs behind is: move
   its data aside (never delete it: it holds the safety log), start it with
   `--bootstrap-from <peer> --catch-up-from <peer> --prune-depth 3600`. The
   safety log restarts empty, which is safe only because the node's last
   signed round is epochs older than any round it can sign again; the runbook
   must check that the network's epoch is greater than the node's last one.
4. **Code, still open:**
   - snapshot serving on by default for any node started with
     `--validator-key`, so a validator cannot be configured into decision 1's
     failure;
   - **done (2026-10-10):** `--catch-up-from` is repeatable; the node probes
     each source once (`RpcBootstrapSource::probe_tip_height`, no retry: a
     dead peer costs ~2 s, not the 92 s retry budget), takes the reachable one
     with the highest tip, and starts as an ordinary restart when none answers
     (`bft::pick_catch_up_source`, two tests);
   - an attacknet round "validator down > 8 epochs" that exercises decision 3
     end to end with real binaries.

## Not chosen

- **Keeping checkpoints for every epoch.** A checkpoint attests a block, but a
  follower still needs the blocks in between to execute them, and validators
  prune to 3600. Checkpoints without blocks do not bring a node back.
- **Automatic re-bootstrap** when catch-up says "restore from a snapshot":
  it would move a validator's data aside unattended. An operator does it.

## Consequences

- Snapshots cost disk on every validator (one state copy per interval kept
  `--snapshot-depth` deep). Measured size: MISSING; record it after the first
  served snapshot.
- The watchdog in `start-testnet.ps1` now matches the binary path under
  `bin\`, so lab or peer processes sharing an image name no longer hide a
  stopped seed.
- The attacknet controller kills lab targets by data directory, and its
  unimplemented faults fail instead of reporting PASS.
