# ADR-052: A follower advances on certificates the current committee signed

**Status:** Accepted (Eric, 2026-10-18; whole-epoch replay confirmed by the
coordinator, 2026-10-18). Implemented: see "Results" and "Built". Mainnet gate 10
(`docs/mainnet-v1-plan.md`) is closed for the case of more than `f` followers
that can replay the current or previous epoch.
**Date:** 2026-10-18
**Amends:** ADR-038 (section "Amendment 2026-10-18").

## Context

ADR-038 lets a node that is more than the engine window behind import blocks only
up to a quorum-attested checkpoint. A checkpoint needs `n - f` attesters, and
followers do not attest, because a follower cannot verify a block above the last
checkpoint. So when more than `f` validators follow at once, the builders are
fewer than `n - f`, no checkpoint forms, and the followers never move.

The 2026-10-18 incident and `bft_follower_tests` show the result. With four
validators and two followers, the builders stop at height 12, the epoch-2
boundary. The followers stay at height 9 in epoch 1, because they switch epoch
only by importing the boundary block, and they drop epoch-2 frames. The chain
halts. Measured: builders height 12 epoch 2; followers height 9 epoch 1,
`follower: true`.

## Decision

A follower advances by **re-deriving blocks from certificates it verifies
itself**, not by importing a peer's blocks on a peer's word.

1. **Certificates carry the evidence.** A `Certificate` is a vertex with `n - f`
   signatures from the committee of its epoch. The committee is computed from
   the follower's own executed state at the epoch boundary (`staked_committee`),
   as builders do. A certificate whose signatures do not reach `n - f` valid
   members of that committee is refused.
2. **Replay, not import.** A follower asks peers for the certificates of its
   epoch after its tip's seal round, from each peer's `certs.log`. It feeds them
   to its own engine. The engine orders them and derives the blocks through the
   same `build_block` and `Chain::insert_block` path as a builder, so
   `insert_block` checks the state root (invariant 24).
3. **Handover.** When the engine's committed round is within the engine window of
   the network's (the rule in "What within window means"), the follower clears
   `follower` and builds, as it does today.
4. **Epoch boundaries.** The follower switches epoch when its own executed state
   names the next committee, exactly as `on_tick` does now. No new rule.

## Threat model

- **Forged commit evidence.** A forged certificate needs `n - f` valid signatures.
  With at most `f` Byzantine members, a forger needs at least `n - 2f` honest
  signatures on a different vertex for the same slot. Honest validators sign once
  per slot (`safety.log`, fsync'd before send), so that cannot happen for a slot
  the honest set has already signed. A forged certificate is refused by the
  engine's own verification, before any block is derived from it.
- **Equivocating minority.** Up to `f` validators may sign two vertices for a slot.
  Each equivocation is evidence (already recorded), and the engine orders by
  certificates, so a replayed equivocation yields no block the honest set would not
  have derived. The `2f + 1` attestation rule stays for checkpoints.
- **Peer withholding blocks or certificates.** A withholding peer stalls the
  follower; it cannot make the follower accept a wrong block. The follower asks
  every configured source, and when none answers it keeps retrying. This is a
  liveness loss for the follower only.
- **Peer lying about its tip.** Only orders the attempts (ADR-043). Trust is never
  placed in the tip.

Why this is safe: every block the follower adds is derived locally from
certificates that carry `n - f` committee signatures, and its state root is
re-executed. A single peer can delay progress but cannot supply content. Followers
never attest what they have not derived, so the ADR-038 objection (one peer's
chain reaching quorum through followers) does not apply.

## Liveness argument

With `n - f` builders up, the chain makes progress and the builders hold the
epoch's certificates in `certs.log`. A follower that asks any one honest peer
that holds the epoch's log receives the certificates it needs and replays them.
The only case that fails is one where no honest peer still holds the epoch's
log. Logs are kept for the current and previous epoch (`store.rs`
`prune_epochs_before`), so this holds for a node at most one epoch behind. A
node further behind still needs a snapshot, as ADR-043 says.

## What "within window" means

Within window: the follower's committed round is within `GC_DEPTH / 2` rounds of
the peer's tip round, in the same epoch, and the follower's tip is an ancestor of
the peer's chain. That is the test `within_engine_window` applies today. Under
this ADR it is checked on each follow round, not only at startup. A follower that
is within window and whose tip is the previous anchor's block hands over to
building.

## Measurement (2026-10-18, gate 1 of the implementation)

Harness: `crates/node/tests/cert_replay_measure.rs` (ignored test). An in-process
committee runs to 3,600 blocks; every distinct certificate is kept; a fresh
observer then replays them. Debug profile, `cargo test`, one machine.

| n | certs | signatures | verify all (s) | replay to 3,600 (s) | verify + replay (s) |
|---|---|---|---|---|---|
| 4 | 28,804 | 86,412 | 18.5 | 32.3 | 50.8 |
| 7 | 50,407 | 252,035 | 30.9 | 34.9 | 65.8 |

Run times (mesh only, not the gate): n=4 1,074 s, n=7 2,556 s. The replay agreed
with the committee at every height (`AGREE`).

Memory: not measured in-process (the Windows commit probe was refused by the
worktree guard). Estimate: an ML-DSA-65 signature is 3,309 bytes (FIPS 204), so
the signatures of one n=7 epoch alone are about 834 MB; n=4 about 286 MB.

**Gate result: n=7 is over 60 s (65.8 s). Stop.** Under the agreed rule the
certificate-replay path is not implemented. The header-root alternative is
written up as ADR-053 (Proposed) for Eric.

## Decision record (2026-10-18)

Eric approved a protocol fix before mainnet. The coordinator decided, within that
approval: **whole-epoch certificate replay** is the mechanism. The 60 s gate used
earlier was an arbitrary threshold and is withdrawn. 50.8 s (n=4) and 65.8 s
(n=7), debug build, is accepted for a one-time rejoin. It is far better than an
unbounded halt, and mainnet launches with n=4. ADR-053 stays Proposed, a future
optimisation, and is not implemented.

## Results (release build, 2026-10-19)

Epoch of 3,600 blocks, one machine, `cargo test --release` of
`cert_replay_measure.rs` (ignored test). Catch-up is `certsync::sync`: paged
fetch from member 0's log, verification of every page before the next, the
per-certificate log write, and the replay.

| n | certs | signatures | verify pass alone (s) | catch-up total (s) | working set before (MB) | peak (MB) | growth (MB) |
|---|---|---|---|---|---|---|---|
| 4 | 28,804 | 86,412 | 22.7 | 47.7 | 32.0 | 37.0 | 4.9 |
| 7 | 50,407 | 252,035 | 35.0 | 98.5 | 76.4 | 78.9 | 2.5 |

The working set is the whole test process, which still holds the reference
committee's chain (member 0), so the absolute peak includes it. The growth
during catch-up is the number that bounds the catch-up. Memory is measured
with `GetProcessMemoryInfo` (psapi FFI, no new dependency). Debug (earlier):
n=4 50.8 s, n=7 65.8 s. The release build is not faster for verification
(22.7 s against 18.5 s in debug), so the release gain is not visible in this
measurement. Not investigated.

**Known cost to fix:** the catch-up flushes the log after every certificate.
Batching the flush per page is the next optimisation; not done here.

## Built

- Store (`consensus/bft/store.rs`): `read_certificate_page` returns a bounded page
  of raw records from a byte offset; `for_each_certificate` streams a log.
  `SafetyStore::open` no longer loads the epoch's certificates.
- Engine restart (`driver.rs`): the replay streams the log through the engine,
  so an epoch's signatures are never all in memory at once.
- Certificate sync (`consensus/bft/certsync.rs`): `verify_certificate` (epoch,
  engine quorum rule, one valid signature per vote), `fetch_epoch` (verify each
  page before the next, refuse no-progress and oversized pages), `sync` (one epoch
  at a time, replays, then the next epoch if the state moved on).
- RPC `get_certificates [epoch, offset, max]`: a blocking method (it reads the
  disk). The server caps `max` at 256 records and the page at 4 MiB of JSON
  (`encoded_len`: hex, quotes and comma). Both ends raise jsonrpsee's response
  limit to 16 MiB, which is at least twice the page, so a full page is never cut.
  Gated by the node's per-client call rate limiter. A per-IP byte budget is NOT
  implemented; the limiter still counts calls.
- Node startup (`bins/maya2c-node`): with `--catch-up-from`, sync from the first
  source that answers, repeat up to 3 times while outside the engine's window. The
  checkpoint path is the fallback when every source refuses.
- Handover: a replayed node is a full engine from the start, not a follower. The
  live tail is served by gossip, as for any restart. Gate 10 is closed by this path.
  Checkpoint catch-up stays for the fallback case.

## Completeness and why it is safe (2026-10-19, review fixes)

Reviews found three gaps in the first version: a peer's `done` ended an epoch
early; a peer could skip a record; and a stalled peer could loop the fetch.
The fixes:

- **Progress and arithmetic.** A page must end where its records end
  (`next = offset + Σ(8 + len)`), and a page that is not the last must carry
  records. Each epoch is capped at 65,536 pages, 4,194,304 records and 4 GiB,
  against about 50,000 certificates for a 3,600-block epoch at n = 7.
- **Completeness is not `done`.** After replaying an epoch, `attested_end`
  takes the epoch's final checkpoint (a quorum-signed block, ADR-038), verifies
  its quorum against the epoch's committee, and requires the replayed history
  to contain that block id at that height. A finished epoch with no checkpoint
  on offer is refused, since nothing then says where it ends. The head epoch
  may have none yet.
- **Why a gap cannot pass.** The block id covers the header: the seal (epoch,
  anchor round), the transactions' root and the state root. If the replayed
  history reproduces the attested id at a height, the committed sequence up to
  that height is the one the quorum signed. Omitting a certificate changes that
  sequence only if the certificate contributes transactions, a seal or a state
  change, and then the id differs and the sync is refused. Omitting one that no
  later certificate names changes nothing, and the replay correctly accepts it.
  A certificate that a later one names as a parent leaves that child buffered in
  the engine, so the replay stops short and the check refuses.
- **Commit rule, for the argument.** The engine (`crates/dag-bft`) is a
  certified DAG: a vertex is accepted with a quorum of parents, and an anchor's
  sub-DAG is committed once the commit rule over later certificates (Bullshark)
  selects it. Commitment depends on which certificates a node holds, not on when
  it holds them, so the replayed sequence equals the network's wherever the
  replay holds the same certificates.
- **Timing.** `now_ms` reaches the engine only for anchor timeouts and resend
  intervals (`validator.rs`: `try_advance`, `tick`). Those decide when a node
  proposes or gives up waiting, not which blocks are committed. Block timestamps
  come from the vertices (`builder.rs`), not from the clock, so the replay on
  `now_ms` does not change block content (invariant 9).
- **Residual risk: the tail.** Blocks above the newest checkpoint are not yet
  pinned by a quorum. A gapped tail would show up as a peer's checkpoint whose id
  differs from the node's block at that height. The follow loop asks a peer for
  its checkpoint about every 30 s (`BUILDER_FOLLOW_EVERY`) and reports a mismatch
  as an error (`catch-up: ... retrying`); it does not halt the node. Making that
  a halt is not implemented. Documented here as the open item.
- **Equivocation at restart.** The safety log is truncated to its last complete
  record before any append (`repair_torn_tail`), so a vote sent after a crash is
  readable. `restore_vote` keeps the first digest recorded for a slot
  (`or_insert`), so a restored vote is not overwritten. Refusing a conflicting
  vote in the engine was not re-tested in this change: NOT VERIFIED.

## Gaps (not covered by this ADR)

- Logs are kept for the current and the previous epoch (`RETAINED_PAST_EPOCHS`).
  A node more than one epoch behind cannot replay and still needs a snapshot.
- Spec vectors for the RPC: NOT YET (`spec/05-consensus.md` documents the wire
  rules without rule IDs, so `spec-coverage --strict` stays green).

- **Replay depth.** The engine's ordered set below the resume point is not
  stored. Replay therefore starts at the epoch's first certificate: O(epoch length).
  Measured in "Results" for a 3,600-block epoch.
- **Certificate serving.** Done as the paged RPC `get_certificates` (see "Built").
- **Safest alternative (ADR-053, Proposed, not implemented).** If replay cost is too high, put the ordered-set root
  into the block header, so the attested checkpoint alone fixes the set. That is a
  consensus format change: it needs a spec update, conformance vectors
  (`cargo xtask spec-coverage`), and a new ADR. It is not part of this ADR.

## Interaction with gate 10

Gate 10 is "more than `f` validators each genuinely need to follow". This ADR
removes the checkpoint requirement for that case. Gate 10 stays open until the
replay path is implemented and `bft_follower_tests` passes with two followers
and a crashed validator (expected to halt, then recover once the crashed one
returns).

## Consequences

- New RPC and a certificate replay path in the node. Consensus-adjacent: rust-reviewer
  and security-reviewer passes before merge (Standing Order 4).
- Deterministic only: no wall-clock time and no `HashMap` iteration in any decision.
  Retry timing is a liveness choice in the node loop, not a consensus rule.
- The operating rule (restart one validator at a time, `chain-health.ps1`) stays
  until this ships. It keeps `f` followers at most, which is the case the existing
  protocol already handles.
