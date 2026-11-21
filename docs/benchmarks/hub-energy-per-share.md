# Hub: joules per accepted share

The Hub shows, for each GPU worker whose card it can identify (below), the
energy the card reported per share the node accepted. Every figure carries the label
**"GPU-reported, not wall-measured, unverified"**, in the UI and in the
`miner_state` report (`workers[].energy`). Eric chose this metric on
2026-11-14 (founder audit question 29, option A); the decision and its
non-goals are [ADR-141](../adr/ADR-141-hub-energy-per-share.md). A wall-meter
calibration (option B) costs money and has not been done.

## What is measured

- **Watts:** `nvidia-smi --query-gpu=power.draw`, read every third second
  while a GPU miner runs (`apps/hub/src-tauri/src/lib.rs`, `supervise`). This
  is the same reading the rig view's Power ring already shows.
- **Joules:** the trapezoid between consecutive readings, so the energy of
  each share interval follows the power through it
  (`apps/hub/src-tauri/src/energy.rs`).
- **Shares:** the accepted shares the miner reported inside those same
  intervals. Rejected shares are not counted. Accepted means the node took the
  share into its pool, not that the chain paid it.
- **Joules per share:** joules divided by shares, since the worker started.
  There is no figure until a share lands in a metered interval.

An interval is left out, with the shares found inside it, when:

- a reading is missing (`[N/A]`, a failed `nvidia-smi`) or out of range
  (not finite, negative, or above 2,000 W, `MAX_WATTS`);
- the worker has no process (stopped, held hot, waiting to restart), or its
  process is not on work: the miner's last event was `idle` (mining not open,
  epoch budget spent) or `error` (RPC down) rather than `work` or `hashrate`;
- the process ended or was relaunched between the two readings, however soon
  the next reading came (a crash inside the restart backoff included);
- the card nvidia-smi lists at the worker's index is not the adapter the
  miner said it opened: the miner's `start` event names its wgpu adapter, and
  the Hub gives the watts only when nvidia-smi's name at the same index is the
  same (ignoring case and padding). wgpu numbers every adapter on every
  backend, integrated GPUs included, so on a machine with more than one
  adapter the two lists often differ, and then no figure is shown;
- the miner has sent nothing for 10 s (`MAX_GAP_MS`): it reports a hash rate
  every 2 s, so a hung kernel or a driver reset stops the meter before the
  60 s stall kill does;
- two readings are more than 10 s apart (`MAX_GAP_MS`). Intervals are timed
  on a monotonic clock, so a wall-clock step (NTP, Windows time sync) never
  stretches one.

Joules and shares therefore always cover the same time: time the miner was on
work and reporting. `metered_ms` says how much time that is.

## How to read it

- It is **the card's own estimate of its board power**. It leaves out the CPU,
  the power supply's losses, fans and the rest of the machine, so a wall
  meter would be expected to read higher if the sensor is accurate. The
  sensor's accuracy is unverified. It includes anything else the card is
  doing (a desktop, a video, a game).
- It is **one run on one machine**. Shares arrive at random, so a run with few
  shares has a wide spread; read `shares` before the figure.
- It is **not comparable** with other chains' energy-per-transaction
  estimates, and nothing here supports an efficiency claim. A share is not a
  transaction or a block.
- **NVIDIA only.** AMD and Intel report no power to the Hub yet.
- **The index is assumed between identical cards.** The name check catches a
  different card at the same index, not two cards of the same model listed in
  a different order by wgpu and by nvidia-smi. On a rig of identical cards,
  read a per-card figure as "one of these cards".
- Energy has **no consensus role**. The figure never leaves the Hub except in
  its own status report, and it never touches a payout.

## Fields (`workers[].energy`, GPU workers only)

| Field | Meaning |
|---|---|
| `joules` | Energy over the metered intervals, J |
| `shares` | Accepted shares inside those intervals |
| `metered_ms` | Total length of the metered intervals |
| `joules_per_share` | `joules / shares`, or `null` before the first metered share |
| `source` | `nvidia-smi power.draw` |
| `label` | `GPU-reported, not wall-measured, unverified` |

No measured figure is recorded here: no Hub run against a mining chain has
been made with this build. When one is, it goes in `reports/bench/` with the
commit, the card, the driver version and the shares counted.
