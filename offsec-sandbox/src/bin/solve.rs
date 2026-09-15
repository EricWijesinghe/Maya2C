//! Boundary-input generation with a solver, over the arithmetic crates only.
//!
//! A research aid, off by default (`--features z3`). Kani is the authority on
//! `ledger-math` — it bounded-model-checks the real code. This does the
//! complementary, weaker thing: encode a boundary condition (an addition that
//! overflows `u64`, a distribution whose remainder is largest) and ask Z3 for
//! inputs that hit it, to seed the fuzz corpus with cases a random mutator would
//! reach rarely.
//!
//! Without the `z3` feature this prints why it is a no-op and exits cleanly, so
//! the binary always builds.

use std::process::ExitCode;

fn main() -> ExitCode {
    #[cfg(feature = "z3")]
    {
        let target = std::env::args().nth(1).unwrap_or_else(|| "ledger-math".to_string());
        match target.as_str() {
            "ledger-math" => {
                maya_offsec_sandbox::engine::solve_ledger_boundaries();
                ExitCode::SUCCESS
            }
            other => {
                eprintln!("no solver model for {other:?}; known: ledger-math");
                ExitCode::from(2)
            }
        }
    }
    #[cfg(not(feature = "z3"))]
    {
        eprintln!(
            "the solver is off: build with `--features z3`. Kani is the authority \
             on ledger-math; this only seeds the corpus with boundary inputs."
        );
        ExitCode::SUCCESS
    }
}
