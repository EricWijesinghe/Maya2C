# benches/

Empty of benchmarks, for the same reason as `tests/` - a virtual workspace
manifest owns no targets, so cargo would not build anything placed here.

They moved with the crate they measure:

| Where | What |
|---|---|
| `crates/node/benches/` | 10 targets: `algo_comparison`, `argon_blake`, `dag`, `dex_matching`, `ebpf_bench`, `gas_predictor`, `hybrid_footprint`, `hybrid_signing`, `mlkem_handshake`, `oracle` |
| `crates/vm/benches/` | `module_cache` |
| `crates/htlc-lattice/benches/` | `verify` |
| `crates/zkml-prover/benches/` | `verify` |

`cargo bench --workspace` runs them; `scripts/bench_report.py` formats the
output.

One standing rule, from the Standing Orders in `CLAUDE.md`: a number in a
brief is a target, not a result. `crates/node/benches/algo_comparison.rs`
states in its own output what it could not measure rather than leaving a
silent gap, and that is the pattern.
