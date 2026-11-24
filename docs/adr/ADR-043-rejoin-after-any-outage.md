# ADR-043: A way back after any outage, not only a short one

**Status:** Accepted (2026-10-10). Applied on maya-testnet-1 and in code;
attacknet attack 7 passes. Mainnet launch gates 8 and 10 (`docs/mainnet-v1-plan.md`).
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
   - **done (2026-10-10):** a node that signs (`--validator-key` or
     `--remote-signer`) snapshots every 3600 blocks unless
     `--snapshot-interval` says otherwise; `0` turns it off
     (`pruning::snapshot_interval`, two tests);
   - **done (2026-10-10):** `--catch-up-from` is repeatable; the node probes
     each source once (`RpcBootstrapSource::probe_tip_height`, no retry: a
     dead peer costs ~2 s, not the 92 s retry budget), takes the reachable one
     with the highest tip, and starts as an ordinary restart when none answers
     (`bft::pick_catch_up_source`, two tests);
   - **done (2026-10-10):** `cargo xtask attacknet --beyond-retention`
     (attack 7) exercises decision 3 end to end with real binaries:
     `validator 5 down from epoch 1 to 10 (591s), back from a snapshot at 604
     in 527.6ms`; no fork through 604 (`reports/attacknet/2026-10-10-6.md`).
     The earlier run (`-5.md`) rejoined too, but failed the harness's no-fork
     walk, which expected every node to hold height 1; it now checks a
     bootstrapped node from the first height it serves.

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

## Amendment 2026-11-04: a source behind its own checkpoint

A start-up catch-up (`--catch-up-from`) from a peer that serves a verified
checkpoint whose block it has not imported yet (a follower one block behind)
used to fail the node: the range request for that block returned "no block at
height N", no other source was tried, and the node exited. Now such a source
imports nothing and the node takes the existing "source behind" path (the
checkpoint-follow path, or resuming as a validator when within the engine's
window). Nothing unverified is imported: the checkpoint is still checked for a
quorum before the height test. Refusals for bad evidence stay errors. The
regression test is `a_source_behind_the_checkpoint_it_serves_imports_nothing_and_is_no_error`
(`crates/node/src/consensus/bft/catchup.rs`). Found by attacknet "restart ahead"
on commit 9913c144 (3 failures in 10 weighted runs, see the PR description).

Known limits, recorded rather than fixed:

- **Checked anchor has no `require_reached` equivalent.** A `--trusted-block`
  start-up that reaches no checkpoint the source holds gets an empty batch,
  and `check_trusted` returns Ok on an empty batch. A stale node is still
  held by `require_reached` (the Stale anchor); a Checked node is not. This is
  acceptable because a Checked node is not stale, and its committee is trusted.
- **A far-behind node can starve on one source.** A follower that is always at
  least one block behind its source's newest checkpoint imports nothing each
  round. Importing up to the tip needs an attested block at or below it, and
  the checkpoint book keeps one checkpoint per epoch (`attest.rs` `of_epoch`),
  so no earlier one is available. Fixing it would weaken ADR-038. Not yet
  filed as an issue.
- **Lying-source starvation** (was pre-existing) is fixed by the source-rotation
  amendment below (decision 5).
- Each empty round logs one `catch-up: ... behind its own checkpoint` line.

## Amendment (2026-11-04): the follower moves between sources

Found in the security review of PR #359. Decision 2 picks one source at
startup, and the follow loop kept reading from it for the rest of the run.
A source that answered the tip probe high, then imported nothing (or served
only old checkpoints, `catchup::fetch_anchored` returning empty), starved the
node without an error: it never followed a wrong chain, because every block
still needed a quorum of attested checkpoints, but it never progressed.

- **Decision 5.** `follow_loop` takes the whole `--catch-up-from` list
  (`bft::SourceRotation`), starting on the source that answered. A following
  round that imports nothing, or fails, counts as stalled. After
  `FOLLOW_ROTATE_AFTER` (60) stalled rounds in a row the loop moves to the
  next source, round-robin, and logs the move. A progressing round resets the
  count. A builder's quiet rounds do not count.
- **Why no trust is needed.** A rotation only changes which peer is asked.
  Each block still passes `catch_up_once`, which checks it against the
  committee and an attested checkpoint, so a rotated-to source can add
  blocks but cannot put a wrong one in.
- **Why 60 rounds.** A source with sparse checkpoints is healthy while it
  goes quiet for a while; rotating on each quiet round would churn between
  honest sources. A minute bounds how long a lying source can hold a node.
- **Cost.** A halted network with no progress logs one rotation a minute per
  source. Accepted: the log is the signal an operator needs.
- **Not changed (known, recorded).** `within_engine_window` still reads the
  source's tip header nonce unverified. A lie there chooses between "resume as
  a full validator" and "follow attested blocks". Following is the safe
  direction; a lie can only cost liveness, and the decision happens at startup
  only. Upgrade: decide from a block already imported and verified.
  `catch_up` also sets the driver to follow mode per attempted source; that is
  a startup-only effect and is left as it was.


## Amendment 2026-11-24: no unanchored start in a production build (audit CON-08)

Eric 2026-11-24 chose option A of the CON-08 brief (PR #680).

- **The mainnet binary has no way around a checkpoint.** `--allow-unconfirmed-start`
  is refused at argument parsing in a `--features production` build
  (`rejoin::unconfirmed_start_permitted`). Test and lab builds keep it for a
  halted test network or an old test genesis.
- **A fresh mainnet node joins a running chain only through a trusted block.**
  A production node that holds only genesis, told by any source that the chain
  is past height 0, is refused without `--trusted-block HEIGHT:HEX`
  (`rejoin::fresh_join`, at start-up and in every follow round). At launch,
  while every source reports genesis or none answers, no block exists to trust
  and none is needed: a long-range forgery needs validators that have unbonded,
  and a chain inside its first unbonding window has none (the time rule above).
  The pruned bootstrap already required the block (ADR-060).
- **The guarantee:** no source, and no set of sources an attacker controls,
  decides which chain a new mainnet node follows. The trust a new node starts
  from is a block its operator learned somewhere else. This is a
  weak-subjectivity checkpoint, the defence proof-of-stake chains use against
  long-range attacks on new nodes.
- **Node policy, not consensus.** Block validity and state roots are unchanged.
  Pinned by `rejoin` unit tests
  `a_fresh_production_node_joins_a_running_chain_only_through_a_trusted_block`,
  `at_launch_a_fresh_production_node_needs_no_trusted_block`,
  `a_non_production_build_keeps_the_operator_choices` and
  `a_production_build_refuses_the_unconfirmed_start_flag`.
- **The two honest unconfirmed starts take the node's own tip** (review of
  #694). With the flag gone, a production node had no way to start in the two
  cases it existed for: (a) a mainnet halted longer than the window
  (`unbonding_epochs × epoch_blocks × min(observed interval, 15 s)`, about
  seven hours at one block per second), where no block above any validator's
  tip exists to trust; and (b) a genesis whose timestamp lies more than the
  window (378,000 s for 7 × 3,600 blocks at 15 s) before launch. A
  `--trusted-block` that names the node's **own tip** (height equal to the
  tip, id equal to the tip's id, checked by `check_against_tip`) now lifts the
  unconfirmed refusal and anchors the start to that tip
  (`Anchor::Checked`). The operator states which chain they stand on, which is
  the trusted-block start the CON-08 decision asked for, and a block below the
  tip still lifts nothing. It is only an answer to "unconfirmed": a source that
  reports a head beyond the window still needs a recent block above the tip.
  For (a) every validator restarts with `--catch-up-from <the others>
  --trusted-block <its tip height>:<its tip id>` (`--trusted-block` needs a
  catch-up or bootstrap source). For (b) the start is `--trusted-block
  0:<genesis id>`, and `cargo xtask genesis-check` (also run by the ceremony
  rehearsal) refuses a genesis whose timestamp is more than the window before
  the launch (`--launch UNIX_SECS`, default the clock), so a launch never needs
  it unless it slips. Genesis is no trusted block for joining a running chain:
  `fresh_join` wants a block above height 0 once a source reports one.
  The guarantee: a halted or late-launched mainnet restarts from the
  production binary, and every start still names the block it trusts. Tests
  `a_halted_network_restarts_when_the_operator_names_this_nodes_own_tip`,
  `a_genesis_signed_long_before_launch_starts_from_its_own_genesis_id`,
  `genesis_itself_is_no_trusted_block_for_joining_a_running_chain`,
  `a_launch_past_the_unbonding_window_of_the_genesis_timestamp_is_refused`.
