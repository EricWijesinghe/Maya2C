# Hub GPU auto-tuner

Status: **stub** (ADR-137, HW-1). Written CI-first. Not compiled or run on a
real card yet. Source: `apps/hub/src-tauri/src/` (`autotune.rs`, `tuning.rs`,
`gpu_control.rs`, `profiles.rs`) and `apps/hub/ui/app.js` (the Tune toggle).

## What the user sees

Every GPU card has a **Tune** toggle. It is off by default and never turns on
by itself. While it is on, the card's line shows the state:

| Line | Meaning |
|---|---|
| `stock` | Not tuning. The card runs its vendor settings. |
| `stock, measuring` | The reference hash rate and draw are being taken. |
| `tuning step n` | Trial `n` is applied and being measured. |
| `tuned +x.x% efficiency` | The search settled on a better setting. |
| `rolled back: <reason>` | The card was returned to stock, and why. |

## How the search works

1. Measure stock: one settle sample, then the mean of three samples.
2. Try the saved seed for this card if one exists. Keep it only if it beats stock.
3. Otherwise step the core clock down, then the power limit down, one notch
   at a time. Each trial is measured the same way. A candidate is kept only
   when efficiency (hash rate per watt) rises by more than 1%.
4. Stop after two candidates in a row fail, or after 24 trials.
5. Apply the best setting and keep watching it.

The search is deterministic: the same samples always give the same trials.

## Limits

These are policy values in `autotune.rs`. They are not measured. Changing one
needs a recorded sweep here first.

| Limit | Value |
|---|---|
| Power floor | 60% of the stock power limit |
| Clock floor | 70% of the stock clock |
| Power notch | 5% of stock |
| Clock notch | 5% of stock |
| Power ceiling | the stock power limit (never above it) |
| Temperature ceiling | the user's setting, capped at 80 C |
| Minimum gain to keep a candidate | 1% |
| Collapse | hash rate below 50% of stock, for two samples in a row |
| Trial budget | 24 trials |

Settings only ever go down from stock. The tuner never asks for more than the
vendor set.

## Rollback to stock

The card goes back to stock on any of these:

- the miner crashed (its restart counter rose)
- the miner stopped
- hash rate collapsed
- the temperature went over the ceiling
- the driver refused a write
- the user turned Tune off
- the Hub closed

A reset that fails is retried every few seconds. The GPU stays marked
"tuned" in `gpu-tuning.json` until a reset works. If the Hub dies while a card
is tuned, the next start resets that card before anything else.

## Backends

| Backend | Control | Notes |
|---|---|---|
| NVIDIA (`nvidia-smi`) | power limit (`-pl`) and clock lock (`-lgc`) | Needs administrator rights on Windows. Uses `nvidia-smi`, not NVML. |
| AMD (sysfs, Linux) | power cap only | Not reachable yet: the telemetry does not discover AMD cards. |
| Read-only | none | Any card with no UUID, and Windows AMD. |
| SIM | simulated | Test builds only. Its log lines start with `SIM`. |

## Profiles

`gpu-tuning.json` in the Hub's data folder. Profiles are keyed by the driver's
device UUID, so a card keeps its profile when the bus order changes. An
unreadable file turns tuning off rather than being overwritten.

## Not verified

- Any real card. The NVIDIA path has not been run against `nvidia-smi` on
  hardware.
- Whether a clock lock at the chosen value is stable under load.
- The administrator-rights failure message on Windows.
- The crash-recovery reset after a real power cut.
- Whether the policy limits above are sensible for real miners.
