# ADR-045: Maya2C Hub, one desktop window for mining and every app

**Status:** Proposed (2026-10-11). Asked for by Eric: "all applications should
be presented in a GUI too, like NiceHash Miner, but with advanced graphical
statistics and attractive visuals".
**Date:** 2026-10-11

## Context

Mining (ADR-044) had only a command line. Wallet and Chat are separate Tauri
apps, and the wider ecosystem (exchange, identity, AI, the field apps) has no
place a newcomer can see at a glance. People choose a coin by what they can
see; a miner who sees only text quits early.

## Decision

1. **A new Tauri 2 app, `apps/hub`, outside the workspace** for the same
   reason as `apps/chat-gui`: `generate_context!` checks the frontend at
   compile time, and its dependency graph stays away from the node's.
2. **The hub never hashes, signs or holds keys.** It runs
   `maya2c-gpu-miner --mine --json` as a child process and reads one JSON
   event per line (`start`, `work`, `idle`, `hashrate`, `share`, `error`).
   The miner owns the payout key file; the hub only shows the address it
   announces. Closing the hub stops the miner, so nothing mines unseen.
3. **Miners get work from the node, not from their own copy of the rules.**
   `get_mining_work(address)` returns the share header with the chain tag,
   anchor and payee already folded in, plus target, DAG profile and epoch.
4. **Only measured numbers are drawn.** Hash rate comes from the miner's own
   count; GPU temperature, power, load, fan, VRAM and clock come from
   `nvidia-smi` and show a dash when the driver reports none. Block rate is
   measured by the hub over its session and labelled so. Opened in a plain
   browser, the UI runs on synthetic preview data and says so in its header
   (Standing Order 3).
5. **The Universe page lists every app and field with an honest status**
   (Live on testnet / Building / Research / Planned), kept in
   `apps/hub/ui/universe.js` and to be kept in step with `features.toml`.
6. **No web libraries.** Canvas drawing and self-hosted fonts (OFL) keep the
   app offline-capable under a `'self'`-only CSP.

## Consequences

- Hashing is CPU light-mode today (about 2 ms a digest per thread). The GPU
  dataset path in `hal/wgpu-miner` replaces the hashing loop later without
  changing the event protocol or the hub.
- AMD and Intel GPU telemetry is not wired.
- Wallet and Chat still open as their own apps; the hub links them in a later
  step rather than duplicating their key handling.
- Status `planned` in `features.toml` until an end-to-end run against a
  mining chain is recorded in `reports/`.
