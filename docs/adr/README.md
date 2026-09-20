# Architecture Decision Records

One file per decision that would otherwise be re-litigated by whoever arrives
next. An ADR records what was decided, what it cost, and what would have to be
true to revisit it — not what the code does. For what the code does, read
`CLAUDE.md` and `docs/architecture-vision.md`.

A record is written **when the decision is made**, not afterwards. If a
decision turns out to be wrong it gets a new ADR that supersedes the old one;
the old one stays, with its status changed, because the reasoning that was
wrong is the useful part.

## Format

```
# ADR-NNN: <title>

**Status:** Proposed | Accepted | Superseded by ADR-NNN
**Date:** YYYY-MM-DD

## Context
What made a decision necessary. Measurements, not impressions.

## Decision
What was chosen, stated so that a reader can tell whether the code obeys it.

## Alternatives considered
Each with the reason it was not chosen.

## Consequences
What this costs, including the parts that are worse than the alternative.

## Revisit when
The observation that would make this worth reopening.
```

## Index

| ADR | Title | Status |
|---|---|---|
| [001](ADR-001-workspace-layout.md) | Workspace layout, and why the directory move is deferred | Accepted |
| [002](ADR-002-feature-tiers.md) | Feature tiers gate services, never consensus | Accepted |
| [003](ADR-003-build-profiles.md) | Build profiles and the disk ceiling | Accepted |
| [004](ADR-004-reality-ledger.md) | `features.toml` as the reality ledger | Accepted |
| [005](ADR-005-dependency-unification.md) | One version per dependency, and the duplication that remains | Accepted |
