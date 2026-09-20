# ADR-004: `features.toml` as the reality ledger

**Status:** Accepted
**Date:** 2026-09-20

## Context

Whether a Maya2C subsystem actually works was recorded in three prose
documents:

- `CLAUDE.md` — a SHIPPED / RESEARCH / PLANNED status table;
- `docs/architecture-vision.md` — the same three tags, per subsystem, with
  paths and rationale;
- `docs/trajectory.md` — the 160-prompt plan, which is explicit that it is a
  plan and not a statement about the tree.

Three copies, maintained by hand, and nothing checked any of them against the
code. `CLAUDE.md` said "32 workspace members" when `cargo metadata` reported
45, and "`target/` is 356 GB" when it was 316 GiB. Both were true once.

The specific failure this guards against is narrower than documentation rot:
a subsystem described as working, with no test that would fail if it stopped
working. In a tree where a third of the subsystems are deliberately dark —
`ZKML_ACTIVATION_HEIGHT = u64::MAX` and its four siblings — "it is in the
tree" and "it runs" are genuinely different claims, and prose does not make
anyone state which one they mean.

## Decision

`features.toml` at the repository root, one `[[feature]]` per subsystem, with:

| field | meaning |
|---|---|
| `id` | stable key, never renamed once referenced |
| `name`, `domain`, `paths` | what it is and where |
| `tier` | `core` / `extended` / `frontier` — see ADR-002 |
| `class` | `REAL` / `SIM` / `RESEARCH` |
| `status` | `planned` / `stub` / `working` / `verified` |
| `tests` | test targets that would fail if it broke |
| `gate` | for the few verified by a command, not a test target |

A `tests` entry is either a path to an integration test or `crate:<package>`
for a crate that unit-tests itself in `#[cfg(test)]` modules — because a
sizeable part of this tree does, and a rule that only counted `tests/*.rs`
would have marked fifteen working subsystems as stubs on its first run.

`cargo xtask coverage` reads the file, prints the table, and **exits non-zero**
when an entry claims `working` or `verified` while naming neither a test that
exists nor a gate. It also rejects an unknown tier, class or status, a
duplicate `id`, a test path that does not exist, a `crate:` reference that
names no workspace package, and a `planned` entry that names tests.

It does **not** run the tests. A green `coverage` means every claim points at
something real; whether that something passes is what the test job says. The
separation is deliberate — this command has to be runnable before anything
compiles.

`docs/architecture-vision.md` stays the authority for **status**. Where the
ledger and the document disagree, the document wins until someone reconciles
them in writing.

## Alternatives considered

**Generate `features.toml` from `architecture-vision.md` on every build.**
Attractive, and how the file was seeded. Rejected as the steady state: the
test mapping is the part with real content, and it does not exist in the
document. A generator would either drop it or keep a second hand-maintained
sidecar, which is the problem again with an extra step.

**Make `features.toml` the single source and generate the prose from it.**
Rejected for now: the prose carries the *reasoning* — why `dex` is
dependency-free, what `STATELESS_ACTIVATION_HEIGHT` costs — which does not fit
in a TOML field and should not be flattened into one. Worth revisiting once
the ledger has been maintained by hand for a while and the duplication becomes
the bigger cost.

**Have `coverage` run the tests it names.** Rejected: it would make the gate
depend on a full build, so it could not run as the cheap first CI step, and it
would duplicate the test job with worse reporting.

## Consequences

- The ledger holds **90 entries**, not the 164 the brief asked for. There is
  no 162-item list in this repository: `docs/trajectory.md` describes 160
  prompts *as twelve domain ranges*, not as individually named deliverables.
  90 is what can be pointed at — the document's 83 tagged rows plus 7
  subsystems it has no row for. Padding to 164 would make the gate report on
  features nobody can find, which inverts its purpose. The file's header says
  this where a reader will meet it.
- It is a fourth place status is written, and it can rot like the other three.
  What makes it different is that `cargo xtask coverage` fails when it rots in
  the one direction that matters.
- Adding a subsystem now means editing `architecture-vision.md`,
  `features.toml` and `CLAUDE.md`. That is one more step than before, and it
  is the step that makes the claim checkable.

## Revisit when

The ledger and the prose documents have drifted twice, or when a generator can
own the mapping without a sidecar — whichever comes first.
