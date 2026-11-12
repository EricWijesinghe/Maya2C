# Hub GPU tuner (guided)

Status: **stub** (ADR-137, HW-1, stages T0 and T1a). Written CI-first: not
compiled or run on a real card yet. Source: `apps/hub/src-tauri/src/`
(`autotune.rs`, `tuning.rs`, `journal.rs`, `profiles.rs`, `driver_tool.rs`)
and `apps/hub/ui/app.js` (the Tune button).

**The Hub never changes your card.** It suggests a power limit, you set it,
and the Hub reads it back and measures. It needs no administrator rights and
has no helper program. Power limit only: no clock offsets, no fan control
(ADR-137 O-1, O-2).

## The steps

1. Start mining on the card with its power limit at the driver default.
2. Set the hash-rate floor slider (default 95 %). The Hub keeps at least that
   share of the hash rate it measures at the default.
3. Press **Tune** on the card. The Hub measures at the default for about a
   minute and a half.
4. The card shows the next limit to try, for example `set the power limit to
   235 W`, and a command you can copy:
   `nvidia-smi -i GPU-… -pl 235`. Run it in an administrator terminal, or set
   the same watts in your GPU tool (for example MSI Afterburner).
5. When the driver reads the new limit back, the Hub waits 30 seconds, then
   measures hash rate per watt for one minute, and shows the next step.
6. It stops at the first step that is no more efficient than the one before,
   or that drops hash rate under your floor, and says which limit to keep.

The search is deterministic: the same readings give the same steps.

## When it stops early

The card shows what happened and what to do: set the power limit back to the
default (the driver's own value, read live, never one from a file). On heat
it says to leave the limit where it is until the card has cooled, because the
default is a higher limit.

| Trigger | Marks the step bad |
|---|---|
| a reading went missing (power, limit, temperature, hash rate) | no |
| the miner stopped | no |
| the miner crashed and restarted | yes |
| the card reached its temperature ceiling minus 3 °C | yes |
| the miner reported a wrong hash (`hw_error`) | yes |
| more rejected shares in a window than at the default | yes |
| the limit reads above the driver default | no |
| the Hub could not write its journal | no |
| you pressed Tune again to stop | no |

A step marked bad is not suggested again for that card and driver version.

## Efficiency read-out

Every mining GPU card shows hash rate per watt drawn (`H/s per W`), from the
miner's latest rate and the driver's power reading. Nothing is shown when
either reading is missing or zero.

## Wrong hashes (`hw_error`)

The GPU miner re-checks every share the card finds on the CPU, with the same
light-mode hash a validator uses. A share the CPU does not confirm is not
submitted; the miner emits `{"event":"hw_error"}` and the Hub counts it. One
during a step stops the search.

## Crash safety

Before a step is shown, the Hub writes it to `gpu-tuning-journal.json` as
`trying` and writes a separate `gpu-tuning.trying` marker, and syncs both.
On the next start, any of these count as "a step was in progress":

- a marker without a matching finished journal entry
- a journal entry still `trying`
- a journal or marker that cannot be read, has unknown fields, or that
  another account could have written

The card's line says to set the default back, and keeps saying so on every
start until you press Tune on that card; then the step is marked bad.
Closing the Hub mid-step counts the same way. Pressing Tune again after an
untrusted journal starts a new one.

## Results

`gpu-tuning.json` keeps the limit each search kept, keyed by the driver's
device UUID and driver version. It is shown, never applied. On load it is
clamped to today's driver range; one from another driver version is ignored.

## Driver tool

`nvidia-smi` is run only by its absolute system path: `System32\nvidia-smi.exe`
on Windows (the folder comes from the Windows API, not an environment
variable) and `/usr/bin/nvidia-smi` on Linux (root-owned, not writable by
group or others, checked before each run). It is never looked up by name, so
a file planted beside the Hub or on `PATH` is never run (ADR-137 S-8). Older
pre-DCH Windows drivers that install it only under `Program Files` are not
found yet.

## Limits

Policy values in `autotune.rs`, not measurements. Changing one needs a run
recorded here.

| Value | Setting |
|---|---|
| Hash-rate floor | slider 80–100 %, default 95 % |
| Step size | 10 % of the driver's range (default minus minimum) |
| Settle after read-back | 10 samples (30 s at the 3 s GPU tick) |
| Measurement window | 20 samples (60 s) |
| Read-back tolerance | 1 W |
| Temperature stop | your ceiling (capped at 80 °C) minus 3 °C |

No step goes above the driver default or below its minimum.

## Not in this build

- Automatic mode (the Hub setting the limit through an elevated helper,
  ADR-137 T1b). Gated on a release signing key and a per-machine install.
- Fans and clock offsets.
- AMD and Intel cards.
- A lock against two Hub windows tuning one card.
- A cap on wrong hashes outside tuning: a card that keeps sending
  `hw_error` keeps mining (each one is refused, never submitted).
- A start refusal when `nvidia-smi` is not at its system path: the miner
  then runs without the thermal stop, and the Rigs card says the tool was
  not found.

## Not verified

- Any real card: nothing here has run against `nvidia-smi` on hardware.
- That `power.default_limit`, `power.min_limit` and `driver_version` are
  reported on every supported driver.
- That the miner's GPU index matches `nvidia-smi`'s index on multi-GPU rigs
  (the hash rate is matched by index; the card itself by UUID).
- Any efficiency gain. It is MISSING until measured on a named card with
  commit, driver, OS and command.
- The crash path by hand: kill the Hub mid-step, restart, confirm the card
  line says to set the default back.
