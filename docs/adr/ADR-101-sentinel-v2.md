# ADR-101: Sentinel v2, phase 1: a dead-man heartbeat and consensus detectors

Status: ACCEPTED for phase 1 (tooling only; `apps/sentinel`; no consensus, wire or state change). Shipped: `apps/sentinel/src/heartbeat.rs`, `liveness.rs`, `detect.rs`. Phases 2 to 5 stay PROPOSED and wait for Eric's answers (see below).
Date: 2026-11-12

## Context

`maya2c-sentinel` (ADR-077, live layer) polls node RPC and pages through ntfy.
Three gaps were left:

1. **Silence looked like health.** If the sentinel crashed, hung, or its
   machine went down, nobody was paged. If every node stopped answering, the
   operator got one `rpc:<node>` warning per node, but nothing said "the
   sentinel cannot see the chain at all".
2. **No consensus view.** The halt detector sees the tip, but not the two
   ways DAG-BFT degrades while blocks still arrive: checkpoints (ADR-038
   quorum attestations) stop forming, or one committee member stops proposing.
   The 2026-10-18 halt started with validators stuck as attested followers.
3. **Stranding never ran against a real node.** `obs.rs` read
   `get_mempool_info.pending`, but the node serves `{size, bytes}`. The tests
   scripted the same wrong shape, so they passed. Every real node was logged
   as `stranding idle`.

The netfault relay (#453, ADR-097) defines the faults the network is tested
under: partition with heal, latency with jitter, and loss as retransmission
stalls. The sentinel had no tests for what it reports under them.

## Decision

All of this is in `apps/sentinel`. The sentinel stays read-only: it adds two
getter calls and no other method. AI proposes, never acts (ADR-077).

- **Heartbeat (dead-man).** `--heartbeat PATH` makes the running sentinel
  write the Unix time after every poll. A second invocation,
  `maya2c-sentinel --check-heartbeat PATH [--max-age-secs 120]`, run from cron
  or Task Scheduler, pushes a Critical `Maya2C sentinel stopped` alert when the
  file is missing, unreadable or older than the limit, and exits 2. It writes
  `PATH.alerted` with the stale stamp it paged on, so a scheduler that runs
  every minute pages once per outage, not once per minute.
- **Blind.** The running sentinel raises `blind` (Critical) when no node has
  answered for `--unreachable-secs`.
- **Finality lag.** `get_checkpoint` gives the newest quorum-attested height.
  `finality-lag` (Warning) when the highest tip is at least
  `--finality-lag-blocks` (default 30, the halt window at the 1 block/s target)
  past the best checkpoint any watched node holds.
- **Per-validator liveness.** `get_bft_status` gives each node's proposal
  round and the committed round. `liveness:<node>` (Warning) when a committee
  member's round has not changed for `--liveness-secs` (default 60) while the
  highest committed round among watched nodes has moved. If the whole network
  stalls, that is a halt, not one page per validator. Observers
  (`validator: null`) are skipped. The body says when the node is a follower.
- **Mempool shape.** `obs.rs` reads `size`, and the tests now script the shape
  the node serves.
- **Fault tests.** `tests/fault_scenarios.rs` replays modelled RPC answers for
  the #453 faults through the detectors and the incident layer, and pins
  which pages are raised, when, and that each raises and resolves once.

## Limits

- Liveness covers only nodes the sentinel polls. No RPC reports another
  validator's proposals, so an unwatched committee member is not covered.
- The fault tests model RPC answers. They do not run the relay. #453's
  end-to-end scenarios have not been run either.
- The `#454` supply-audit detector is not built: #454 was open, not merged,
  on 2026-11-12. It is a phase 1 follow-up.
- A dead-man needs a second scheduler. If the machine that runs both is down,
  nothing pages. Run the checker on a different machine from the sentinel.

## Consequences

- Each poll makes two more RPC calls per node.
- Phases 2 to 5 (ADR-077 sentinel AI) wait for Eric's answers: a local model
  or an API model (an API model costs money), and whether quorum sentinels run
  on other machines.
