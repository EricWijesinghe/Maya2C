# formal/

Empty of crates, on purpose.

The foundation brief listed `formal/` for "Lean 4, Kani, Aeneas". Here is what
this tree actually has, and why none of it is a crate in this directory:

**Kani — in use, and it lives in the crates it verifies.** Proofs are
`#[cfg(kani)]` modules inside `crates/ledger-math`, `crates/dex`,
`crates/governance`, `crates/fee-market`, `crates/threat-intel`,
`crates/htlc-lattice` and `hal/iot-anchor`. That placement is not incidental:
Kani compiles a crate together with its whole dependency graph, which is the
reason those crates are dependency-free at all (invariants 1, 6, 12). Moving a
proof into a separate crate here would mean that crate depending on the one it
verifies, which puts the dependency back.

`cargo kani -p <crate>` runs them; the `kani` job in
`.github/workflows/nightly.yml` runs all seven.

**Lean 4 — PLANNED.** No code in this tree. `docs/architecture-vision.md`
tags it, and `grep` will not find it.

**Aeneas — not evaluated.** It translates Rust to a pure functional model for
proof assistants, which is a different bet from Kani's bounded model checking.
Nobody here has assessed it, and saying so is more useful than an empty
directory implying somebody had.

This directory exists so that the first person to write a Lean proof has an
obvious place to put it, and so that the absence above is written down rather
than inferred from an empty `ls`.
