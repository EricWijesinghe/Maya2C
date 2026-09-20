# formal/lean

Empty. Lean 4 is **PLANNED** - `docs/architecture-vision.md` tags it, and
`grep` will not find it.

What Lean is wanted for, stated so the first proof is aimed rather than
decorative: the properties bounded model checking cannot reach. Kani proves
statements about `u64` arithmetic over a bounded input space; it cannot prove
that the ledger conserves value over an unbounded sequence of blocks, or that
the reorg path restores exactly what it undid.

Those two are the candidates. Neither is started.
