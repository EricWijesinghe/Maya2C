# ADR-141: Hub energy metric: joules per accepted share from GPU-reported power, labelled unverified

**Status:** Proposed, 2026-11-14 (Eric chose option A of founder audit question 29, "I approve everything", Eric 2026-11-14)
**Date:** 2026-11-14
**Related:** [ADR-045](ADR-045-maya2c-hub.md) (Hub), [ADR-073](ADR-073-hub-multi-rig-pool-profitability.md) (per-device workers, watchdog), [ADR-004](ADR-004-reality-ledger.md); how to read the figure: [docs/benchmarks/hub-energy-per-share.md](../benchmarks/hub-energy-per-share.md)

## Context

The founder audit (question 29) asked for an energy figure a miner can read,
without the green or zero-waste claims the project has no evidence for. The
Hub already reads `nvidia-smi` every third second while a GPU miner runs, for
the thermal stop, and that read includes `power.draw`. A wall meter would be
the honest measurement, but it costs money (option B) and is not done.

## Decision

1. **Metric.** Per GPU worker: joules divided by accepted shares, both over
   the same metered intervals, since the worker was started. "Valid share" in
   the audit means the shares the node accepted, the count the Hub already
   keeps from the miner's `share` events with `accepted: true`. Rejected
   shares are not counted.
2. **Source.** `nvidia-smi --query-gpu=power.draw`, the board's own estimate,
   from the supervisor's existing 3 s read. No extra process, no new
   dependency. NVIDIA only.
3. **Integration.** The trapezoid between consecutive readings
   (`apps/hub/src-tauri/src/energy.rs`). Shares count when they land between
   two readings that are both metered.
4. **Interval rules.** An interval is left out, with the shares inside it,
   when:
   - a reading is missing, not finite, negative or above `MAX_WATTS`
     (2,000 W, a corrupt line);
   - the worker has no process, or its process is not on work (the miner's
     last event was `idle` or `error` rather than `work` or `hashrate`);
   - the process ended or was relaunched between the two readings (a crash,
     a stall kill, a stop or a thermal hold breaks the stretch at once);
   - the readings are more than `MAX_GAP_MS` (10 s) apart, or the clock
     stepped back.
5. **Label.** Every figure carries "GPU-reported, not wall-measured,
   unverified" and its source, in the UI and in the `miner_state` report
   (`workers[].energy`).
6. **Timestamp.** The interval is stamped after `nvidia-smi` returns, so a
   slow read does not shift the interval or the supervisor's own clock.

## Non-goals

- **No consensus role.** The figure never leaves the Hub except in its own
  status report, never touches a payout, and no node code reads it.
- **No efficiency claim.** No green, zero-waste, or comparison with another
  chain's energy per transaction. A share is not a transaction or a block.
- **No measured figure is published** until a run against a mining chain is
  recorded in `reports/bench/`, and none is called measured until a wall-meter
  calibration (option B).

## Consequences

- Idle time, downtime and gaps lower `metered_ms` instead of inflating the
  joules, so a figure always covers time the card was hashing. `metered_ms`
  and `shares` say how much evidence a figure rests on.
- The sensor's accuracy against a wall meter is unknown. It excludes the CPU,
  power-supply losses and the rest of the machine, and includes anything else
  the card is drawing for.
- Revisit when AMD or Intel cards report power to the Hub, or when a wall
  meter is available.
