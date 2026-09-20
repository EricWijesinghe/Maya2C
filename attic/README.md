# attic/

Empty, and that is the finding.

The foundation brief asked for a merge plan for four duplications, and said:
*"Do not delete work: move superseded code to `attic/` with a README saying
what replaced it."* Three of the four do not exist in this tree, and the
fourth is not a duplication:

| Claimed | Actual |
|---|---|
| two `state_pruner` crates | **One**, and it is not a crate — `crates/node/src/state_pruner/` is a module |
| several governance modules | **One crate**, `crates/governance`, dependency-free for Kani (invariant 12), plus its state-execution module in the node |
| three explorers | **One.** `apps/explorer` is the chain explorer, `apps/dashboard` is a Leptos ops page, `apps/wallet-gui/ui` is the wallet frontend — three surfaces, three jobs |
| many genesis binaries | **Two**, with different jobs: `genesis` writes a genesis file, `genesis-ceremony` runs the multi-party key ceremony |

The evidence is `reports/00-inventory.md` §6, taken from `cargo metadata`
rather than from reading directory names.

So nothing was superseded and nothing was moved here. This directory exists
because the instruction not to delete work outlives the audit that found none
to delete: the next consolidation has an obvious place to put what it
replaces, and the rule that comes with it is the brief's — a README entry
saying what replaced what, not a silent `git rm`.

The real redundancy in this tree was never duplicate crates. It was dependency
versions: 95 external dependencies across 45 manifests with no
`[workspace.dependencies]` table and six declared at incompatible versions.
That is fixed in place, not by moving anything here —
`docs/adr/ADR-005-dependency-unification.md`.
