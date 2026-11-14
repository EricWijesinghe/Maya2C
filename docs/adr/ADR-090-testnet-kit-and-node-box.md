# ADR-090: Testnet kit and node-in-a-box live in xtask; `down` stops by PID and start time

Status: Accepted (tooling only; no consensus, wire or state change)
Date: 2026-11-10

## Context

Two catalog entries, `testnet-kit` and `node-in-a-box`, need one command each:
a local devnet with its endpoints and an attack lab, and a guided node setup
with a health check. Two questions had to be settled.

1. **Where the code lives.** `maya2c-cli` is the developer CLI and links the
   whole node. The devnet (`up`, `down`, `localnet`) and the attack lab
   (`attacknet`) already live in `xtask`, and operator scripts sit beside them.

2. **How `down` finds what it started.** An earlier stop-by-name rule killed
   the live seed node four times on 2026-10-12 (`taskkill /IM maya2c-node`).
   A PID alone is not enough either: the OS reuses PIDs, so a recorded number
   can later name a foreign process.

## Decision

- Both features are `xtask` subcommands: `cargo xtask kit ...` and
  `cargo xtask node ...`. Setup and health code stays out of `maya2c-cli`,
  which would otherwise gain a node dependency it does not need.
- `up` records each started process as `{label, pid, started}`, where
  `started` is the OS start time read immediately after the spawn
  (`Get-Process ... StartTime` on Windows, `/proc/<pid>/stat` field 22 on Unix).
- `down` never matches by executable name. For each record it reads the
  process's current start time and stops the PID only on an exact match.
  A different start time, or no recorded start time, means the process is
  not ours and is left alone. The decision is a pure function
  (`up::action`) and is tested with a foreign PID that must not be killed.
- `up` refuses to start on busy service ports, so a second devnet cannot
  dial a live network on the same machine.
- `node setup` never installs a service. It prints a systemd unit or a
  `schtasks` command for the operator to apply. Key files are restricted to
  the operator's account and SYSTEM on Windows (icacls), or `chmod 600` on
  Unix.
- The explorer keeps its cargo binary name (`explorer`). The live
  `start-testnet.ps1` copies the built binary to `maya-explorer.exe`, so a
  rename would break that deploy path, and name matching is no longer used.

## Consequences

- Stopping a devnet is safe against PID reuse, reboot and foreign processes.
  Its cost is one start-time query per service on `down`.
- A `up.json` written before this change has no start times, so `down`
  refuses to stop those processes. The operator stops them by hand.
- Host thresholds (20 GiB free disk, 4 GiB RAM) are policy floors, not
  measurements. They should be revised once chain growth is measured.
- The tooling is unit-tested. `kit up` and `node setup` have not yet been run
  end to end; the catalog and ledger record them as stubs.

## Alternatives considered

- **Match by executable name plus PID.** Rejected: the name is shared with
  unrelated processes (Windows' own `explorer.exe`, and any other copy of the
  node), which is the failure this ADR exists to prevent.
- **Put the kit in `maya2c-cli`.** Rejected: it would add the node crate to a
  developer tool that needs only the process and file helpers.
