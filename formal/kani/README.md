# formal/kani

Empty **on purpose**, and this is the interesting one.

Kani is in use - but the proofs live in the crates they verify, as
`#[cfg(kani)]` modules in `crates/ledger-math`, `crates/dex`,
`crates/governance`, `crates/fee-market`, `crates/threat-intel`,
`crates/htlc-lattice` and `hal/iot-anchor`.

That placement is load-bearing. Kani compiles a crate together with its whole
dependency graph, which is *why* those crates are dependency-free
(`docs/invariants.md`, invariants 1, 6 and 12) - one dependency on RocksDB's
C++ and `cargo kani -p maya-dex` stops being possible. A proof crate in this
directory would have to depend on the crate it verifies, which puts a
dependency back into exactly the graph that was kept clean for this.

`cargo kani -p <crate>` runs them. `.github/workflows/nightly.yml` runs all
seven.
