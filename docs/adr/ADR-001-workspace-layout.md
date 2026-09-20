# ADR-001: Workspace layout, and why the directory move is deferred

**Status:** Accepted
**Date:** 2026-09-20

## Context

The workspace is 45 members laid out flat at the repository root, alongside
nine crates that are not members, four of the six documentation and asset
directories, and the root package's own `src/`. `ls` at the root returns 90
entries. There is no grouping by role: `dex/` (a Kani-verified arithmetic leaf)
sits beside `deploy/` and `docs/`.

The foundation brief asked for a conventional layout:

```
crates/  bins/  apps/  hal/  sdks/  formal/  infra/  sim/  fuzz/  xtask/  docs/
```

That is a better layout. It is also, in this tree, a large and uniquely
expensive change:

- **Every `path =` dependency changes.** 45 manifests, plus the nine
  non-members that reference members by relative path.
- **Every fingerprint changes.** `target/` was measured at 316 GiB on a volume
  with 85.6 GiB free; a cold rebuild is roughly 52 minutes on the machine this
  was written for.
- **The root package stops being the root package.** `custom-l1-node` has a
  `[package]` table *and* the `[workspace]` table. Moving it under `bins/`
  makes the root a virtual manifest, which relocates all thirty
  `[profile.dev.package.*]` overrides — including the `line-tables-only`
  override that is the only reason the node binary links on Windows at all
  (`LNK1140`).
- **Paths are referenced from outside cargo**: `CLAUDE.md`, `.ignore`,
  `.claude/settings.json`, five GitHub workflows, `scripts/`, two Dockerfiles,
  `docker-compose.yml`, `k8s/`, `deploy/`, `terraform/`, and the brand-asset
  deployment scripts that walk each app's `dist/`.

Doing that in the same change as the workspace tables, the lint policy, the
profiles and the CI skeleton would mean that when something broke, nothing
would say which of the six changes broke it.

## Decision

Split the foundation work in two.

**Phase A — the governance layer, at current paths.** `[workspace.package]`,
`[workspace.dependencies]`, `[workspace.lints]`, the profile block,
`rust-toolchain.toml`, `.cargo/config.toml`, `xtask/`, `features.toml`, the
ADR log, `PROGRESS.md`, the CI skeleton. All of it works regardless of where
the directories sit.

**Phase B — the physical move, as one mechanical commit containing nothing
else.** Its acceptance test is that `cargo metadata --format-version 1`
reports an identical package set before and after, modulo `manifest_path`, and
that a grep for every old path string across all non-`target` files returns
nothing.

`xtask/` and `sim/` are created at the root in their conventional positions,
because they are new and cost nothing to place correctly.

## Alternatives considered

**Move first, then add the tables.** Rejected: it front-loads the 52-minute
rebuild and the path rewrite before any of the machinery that would catch a
mistake — no workspace lint table, no CI, no `cargo xtask coverage` — exists.

**Move and add the tables together.** Rejected for the attribution reason
above. A workspace this size gives one bisect point per commit; spending it on
six changes at once is wasteful.

**Never move.** Tempting, and it is what "if it compiles, leave it" would
advise. Rejected because the flat layout is already costing something
measurable: nine of the 54 manifests are *not* workspace members and nothing
in the directory structure says which nine, so a contributor has to read
`Cargo.toml`'s member list to know whether `cargo test --workspace` covers the
crate they just edited.

## Consequences

- The repository root stays cluttered until Phase B. Anyone reading it in the
  meantime gets the member list from `Cargo.toml` or `cargo xtask coverage`.
- Phase B will be a diff of several hundred files that is almost entirely
  mechanical, and must be reviewed as such — by running `cargo metadata`, not
  by reading it.
- The 52-minute rebuild is paid twice: once for the profile change in Phase A,
  once for the move in Phase B. Accepted, because paying it once for a change
  that fails opaquely is worse.

## Revisit when

Phase B lands, or when a new subsystem would be the tenth non-member crate —
at which point the layout is doing active harm rather than passive clutter.
