# formal/

Empty of crates, on purpose; the Lean and Z3 models below are not crates.

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

**Lean 4 — hand-written models, checked.** `formal/lean/Maya2C/` holds two
models written in core Lean 4 (no Mathlib), checked with Lean 4.20.0:

- `FeeSplit.lean` — `maya_fee_market::split`; proves `split_sums` (burned +
  treasury + tip = base + tip: the split is exactly 100%) and
  `treasury_bounded`. It also evaluates a 49-line vector grid into
  `vectors/fee_split.txt`, and `crates/fee-market/tests/lean_differential.rs`
  checks the Rust function against every line — the link between the model
  and the code it describes.
- `Supply.lean` — the account transfer transition over `n` accounts; proves
  `transfer_conserves` (a transfer between distinct accounts preserves the
  total whether it succeeds or is refused: no tokens from nothing) and
  `refused_transfer_is_noop`.

Check them: `cd formal/lean && lean Maya2C/FeeSplit.lean && lean Maya2C/Supply.lean`
(no output means every theorem checked). Regenerate the vectors:
`lean --run Maya2C/FeeSplit.lean > vectors/fee_split.txt`.

**These are transcriptions, not extractions.** A model proven correct says
nothing about the Rust unless the two agree; the differential vectors are
that agreement for `split`, and review is all there is for `Supply`.

**Aeneas — evaluated, not wired.** Aeneas translates Rust (via Charon's
MIR export) into Lean automatically, which is what would retire the
transcription caveat above. Charon pins its own nightly and the pure
state-transition core of this tree lives inside `custom-l1-node`, whose
dependency graph (RocksDB, libp2p, wasmtime) Charon would have to export.
The dependency-free leaves (`ledger-math`, `fee-market`, `dex`) are the
realistic first targets. Not done here.

**Z3 — SMT proofs over bounded-integer models.** `formal/z3/fee_market.py`
proves ten statements about `split` and `next_base_fee` for every input in
the u64 domain (no u128 overflow, burned + treasury = base, a share above
100% is clamped, never below the floor, direction of movement, one step
bounded by parent/denom + 1 at up to 2x target). A timeout prints UNKNOWN,
never PROVED. `pip install z3-solver && python3 formal/z3/fee_market.py`.

This directory exists so that the first person to write a Lean proof has an
obvious place to put it, and so that the absence above is written down rather
than inferred from an empty `ls`.
