# Repository map

The one page to crawl from. Every top-level directory, what it is for, and
which document is the authority for it. If a directory is not listed here,
that is a defect — fix this page in the same change ([HYGIENE.md](HYGIENE.md)).

## Reading order for a newcomer

1. [README.md](../README.md) — what Maya2C is.
2. [CLAUDE.md](../CLAUDE.md) — standing orders, invariants, build rules.
3. [STATE.md](../STATE.md) — where the project stands *now* (handover at the top).
4. [docs/architecture-vision.md](architecture-vision.md) — what exists vs. what is planned; the authority for status.
5. [features.toml](../features.toml) — machine-readable "does it work"; `cargo xtask coverage` gates it.
6. [docs/README.md](README.md) — the index of every design document.

## Which file answers which question

| Question | Authority |
|---|---|
| Does feature X work? | `features.toml` (never prose) |
| Why is it designed this way? | `docs/adr/` (one ADR per decision), `DECISIONS.md` (short ones) |
| What must never break? | `docs/invariants.md` |
| What is done / next / deferred? | `PROGRESS.md` / `STATE.md` "Do next" / `BACKLOG.md` |
| What could go wrong? | `RISKS.md`, `THREAT_MODEL.md` |
| Is it ready to launch? | `LAUNCH.md` (gates), `READINESS.md` (generated), `docs/mainnet-readiness.md` |
| How is the wire/state defined? | `spec/` |
| How do I build / run? | `docs/build.md`, `docs/environment.md`, `docs/runbooks/` |

## Source code

| Dir | Contents | Notes |
|---|---|---|
| `crates/` | 58 library crates; `crates/node` is the node | consensus-critical: read `docs/invariants.md` first |
| `bins/` | executables: `maya2c-node`, `maya2c-cli`, miners, pool, genesis tools, attacknet | thin; logic belongs in `crates/` |
| `apps/` | user-facing apps: wallet-gui, chat(-gui), hub, explorer, faucet, status, dashboard, portal, dev-hub | several are nested workspaces with their own `target/` |
| `hal/` | hardware/transport: ebpf-net, radio, entropy, miners (cuda/wgpu), iot, simulators | simulators declare themselves (Standing Order 3) |
| `sdks/` | client SDKs: go, python, js, wasm, ffi | |
| `contracts/` | example on-chain contracts (nft-game, token-swap) | built wasm lives in `target-contracts/` (tracked on purpose) |
| `econ/`, `sim/` | economic and network models | models, not production paths |
| `xtask/` | `cargo xtask …` — coverage, status, sweep, disk, release-check | the project's own tooling, in Rust |

## Verification and specification

| Dir | Contents |
|---|---|
| `spec/` | normative protocol spec + conformance vectors |
| `formal/` | Kani, Lean, Z3, Aeneas proofs |
| `fuzz/` | cargo-fuzz targets and corpus (nested workspace) |
| `benches/`, `benchmarks/` | competitive benches; accepted-regression list for the bench gate |
| `tests/` | intentionally empty (virtual workspace); see its README |
| `supply-chain/`, `deny.toml` | `cargo vet` (report-only), `cargo deny` (blocking) |

## Operations

| Dir / file | Contents |
|---|---|
| `infra/` | terraform, ansible, k8s, helm, docker, per-environment kits (see `infra/README.md`) |
| `infra/monitoring/` | Prometheus, Alertmanager, Vector, Grafana configs + `docker-compose.monitoring.yml` (attacknet lab) |
| `scripts/` | shell/python/PowerShell helpers; `deploy-production.sh` (dry-run by default); `scripts/dev/` = local launcher scripts |
| `chain/` | public testnet descriptor `maya2c-testnet.json` (referenced by DECISIONS, explorer, migration docs) |
| `.github/` | CI workflows |

## Documentation

| Dir | Contents |
|---|---|
| `docs/` | design docs, indexed by `docs/README.md` |
| `docs/adr/` | architecture decision records, `ADR-001`…, never renumbered |
| `docs/plans/` | working plans (NextGen update, optimization) — plans, not status |
| `docs/runbooks/`, `docs/migration/`, `docs/releases/` | operations, upgrades, release notes |
| `docs/master-prompts/` | the original thirty briefs (three files) and the DONE WHEN verdicts |
| `docs/runbooks/` | includes `attacknet-lab-guide.md` |
| `docs/site/` | the documentation website (Astro; `node_modules/` and `dist/` are ignored) |
| `docs/brand/`, `logo-assets/`, `design/` | brand rules, assets, design tokens |
| `reports/` | numbered topic reports `00`–`32`, plus `sessions/`, `executive/`, `sweeps/`, `attacknet/`, `incidents/` — evidence, append-only |
| `attic/` | deliberately empty; its README records why |

## Agent / tool configuration (not product code)

| Path | Contents | Tracked |
|---|---|---|
| `.claude/` | agents, commands, hooks, settings; `plans/` and `skills/` | partly |
| `.agents/`, `.serena/`, `.idea/`, `.posit/`, `.tabnine/`, `.config/` | local tool state | ignored or tool-owned |
| `.mcp.json` | MCP server list (see `docs/environment.md`) | yes |

## Generated or machine-local (never hand-edit, never commit)

`target/`, nested `target/` dirs, `dist/`, `docs/site/{node_modules,dist,.astro}`,
`skills-lock.json`, `skills/`, `*.bak-*`.

Tracked but generated: `READINESS.md` (`cargo xtask readiness`) and
`lint-debt.txt` (the clippy ratchet baseline, `scripts/lint_debt.sh --update`).
