# Prior art: GPU auto-tuning for mining efficiency inside a miner GUI

**Search status:** partially searched 2026-11-12 (web search; sources cited below). Not searched: HiveOS, Awesome Miner and minerstat tuning features in detail; MSI Afterburner's OC Scanner.

Feature: HW-1, [ADR-137](../adr/ADR-137-hardware-intelligence.md).

**Queries:** "NiceHash QuickMiner OCTune auto-tune efficiency GPU power limit optimize"; "NVML nvmlDeviceSetPowerManagementLimit nvmlDeviceSetClockOffsets nvmlDeviceSetFanSpeed_v2 documentation"; "AMD ADLX SDK IADLXGPUTuningServices manual power tuning fan tuning auto tuning interface"; "Microsoft Defender flags WinRing0 driver fan control hardware monitoring tools 2025".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| NiceHash QuickMiner OCTune | In-miner tuning of core clock, memory clock, power limit and fan; "Optimize" profiles; a documented manual method of lowering the power limit stepwise while watching kernel times; write access needs an auth token; settings are restored when the miner exits. | [OCTune wiki](https://github-wiki-see.page/m/nicehash/NiceHashQuickMiner/wiki/OCTune), [FAQ](https://github.com/nicehash/NiceHashQuickMiner/wiki/FAQ) |
| NVIDIA NVML | Documented calls to set a power limit, a fan speed and clock offsets, version-dependent; a forum report says restoring automatic fan control did not behave as documented on one driver. | [Device commands](https://docs.nvidia.com/deploy/nvml-api/group__nvmlDeviceCommands.html), [deprecated list](https://docs.nvidia.com/deploy/nvml-api/deprecated.html), [forum report](https://forums.developer.nvidia.com/t/nvmldevicesetdefaultfanspeed-v2-does-not-resume-fan-speed-algorithm-please-fix/214430) |
| AMD ADLX | Tuning services with support checks, manual power tuning (limit and range), manual fan tuning, automatic tuning profiles, reset to factory. | [IADLXGPUTuningServices](https://gpuopen.com/manuals/adlx/adlx-sdk-references/adlx-interfaces/gpu-tuning/iadlxgputuningservices/), [IADLXManualPowerTuning](https://gpuopen.com/manuals/adlx/adlx-sdk-references/adlx-interfaces/gpu-tuning/iadlxmanualpowertuning) |
| Fan and monitoring tools on WinRing0 | From March 2025 Microsoft Defender flagged the WinRing0 driver (a known-vulnerable driver) and quarantined it, breaking several fan, RGB and monitoring tools. | [Neowin](https://www.neowin.net/news/windows-1110-is-flagging-winring0-on-your-pc-monitoring-fan-control-apps-heres-why/), [Slashdot summary](https://it.slashdot.org/story/25/03/14/1351225/windows-defender-now-flags-winring0-driver-as-security-threat-breaking-multiple-pc-monitoring-tools) |

## What Maya2C has (checked in repo, 2026-11-12)

- Read-only NVIDIA telemetry via `nvidia-smi`: `apps/hub/src-tauri/src/gpu.rs`.
- A thermal stop with hysteresis and a crash supervisor: `apps/hub/src-tauri/src/watchdog.rs`.
- No write to any GPU setting.

## How the Maya2C design differs

- **Narrower on purpose.** First release lowers the power limit only; it
  never raises power or clocks. OCTune exposes more levers.
- **Crash rollback through a write-ahead journal**, so a setting that takes
  the machine down is not applied again. The search did not establish what
  other tools do after a hard crash.
- **No kernel driver and no undocumented interface**, because of the
  WinRing0 history above.
- Restoring the original value on exit matches what QuickMiner documents.

**Verdict: similar exists and is more capable (OCTune). The differences are
restraint and rollback, not capability. No comparative claim is authorised.
Any efficiency gain is MISSING until measured.**
