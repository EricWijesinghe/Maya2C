//! Optional engines: the LibAFL coverage-guided fuzzer, and the Z3 boundary
//! solver. Both are feature-gated so the base crate — mutators, oracle, triage,
//! the working random-search runner — builds and tests without either.
//!
//! # Coverage
//!
//! Full edge coverage needs the target compiled with sanitizer-coverage, which
//! is what `cargo-fuzz` sets up for the `fuzz/` libFuzzer targets. This engine
//! runs LibAFL's in-process fuzzer with our structure-aware mutators, oracle
//! and on-disk corpus; wiring sancov edge feedback into the node crate's build
//! is the next step and is tracked in `docs/offsec-sandbox.md`. Until then the
//! LibAFL path is a stronger scheduler and mutator over the same corpus, not a
//! coverage-guided search — and this module says so rather than implying more.

#[cfg(feature = "libafl")]
mod afl {
    use std::process::ExitCode;

    use libafl::corpus::{InMemoryCorpus, OnDiskCorpus};
    use libafl::events::SimpleEventManager;
    use libafl::executors::{ExitKind, InProcessExecutor};
    use libafl::feedbacks::{ConstFeedback, CrashFeedback};
    use libafl::inputs::{BytesInput, HasTargetBytes};
    use libafl::monitors::SimpleMonitor;
    use libafl::mutators::{HavocScheduledMutator, havoc_mutations};
    use libafl::schedulers::QueueScheduler;
    use libafl::stages::StdMutationalStage;
    use libafl::state::StdState;
    use libafl::{Evaluator, Fuzzer, StdFuzzer};
    use libafl_bolts::AsSlice;
    use libafl_bolts::rands::StdRand;
    use libafl_bolts::tuples::tuple_list;

    use crate::runner::Config;
    use crate::{mutate, oracle};

    /// Runs the LibAFL in-process fuzzer for `config.surface`.
    pub fn run(config: &Config) -> ExitCode {
        oracle::quiet_panics();
        let surface = config.surface;

        // The harness: an input is a crash to LibAFL exactly when the oracle
        // calls it a finding.
        let mut harness = |input: &BytesInput| {
            let target = input.target_bytes();
            if oracle::check(surface, target.as_slice()).is_finding() {
                ExitKind::Crash
            } else {
                ExitKind::Ok
            }
        };

        // No sancov map yet (see the module docs), so no coverage feedback.
        // Crashes are still solutions; the scheduler and mutators do the work.
        let mut feedback = ConstFeedback::new(false);
        let mut objective = CrashFeedback::new();

        let solutions = OnDiskCorpus::new(config.out_dir.clone()).expect("solutions corpus dir");
        let mut state = StdState::new(
            StdRand::with_seed(config.seed),
            InMemoryCorpus::new(),
            solutions,
            &mut feedback,
            &mut objective,
        )
        .expect("state");

        let monitor = SimpleMonitor::new(|line| println!("{line}"));
        let mut manager = SimpleEventManager::new(monitor);
        let scheduler = QueueScheduler::new();
        let mut fuzzer = StdFuzzer::new(scheduler, feedback, objective);

        let mut executor = InProcessExecutor::new(
            &mut harness,
            tuple_list!(),
            &mut fuzzer,
            &mut state,
            &mut manager,
        )
        .expect("in-process executor");

        // Prime the corpus with the structure-aware seeds, so the mutational
        // stage has valid starting points rather than only random bytes.
        for bytes in mutate::seeds(surface) {
            let _ = fuzzer.add_input(
                &mut state,
                &mut executor,
                &mut manager,
                BytesInput::new(bytes),
            );
        }

        let mutator = HavocScheduledMutator::new(havoc_mutations());
        let mut stages = tuple_list!(StdMutationalStage::new(mutator));

        // Time-bounded runs use fuzz_loop_timeout; iteration-bounded ones loop.
        let result = fuzzer.fuzz_loop(&mut stages, &mut executor, &mut state, &mut manager);
        match result {
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("libafl: {error}");
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(feature = "libafl")]
pub use afl::run;

#[cfg(feature = "z3")]
mod solver {
    use z3::{SatResult, Solver, ast};

    /// Finds and prints `u64` pairs whose sum overflows — the boundary a random
    /// mutator reaches rarely, seeded here for the corpus. Kani proves the real
    /// `ledger-math` code refuses these; this only enumerates witnesses.
    pub fn solve_ledger_boundaries() {
        // z3 0.21 keeps one context per thread; there is no context to pass.
        let solver = Solver::new();

        let a = ast::BV::new_const("a", 64);
        let b = ast::BV::new_const("b", 64);
        // Unsigned overflow of a + b: the sum wraps below one of the addends.
        let sum = a.bvadd(&b);
        solver.assert(sum.bvult(&a));
        // A non-trivial witness.
        solver.assert(a.bvugt(ast::BV::from_u64(0, 64)));

        match solver.check() {
            SatResult::Sat => {
                let model = solver.get_model().expect("model");
                let av = model.eval(&a, true).and_then(|v| v.as_u64()).unwrap_or(0);
                let bv = model.eval(&b, true).and_then(|v| v.as_u64()).unwrap_or(0);
                println!("ledger-math overflow witness: {av} + {bv} wraps");
            }
            other => println!("no overflow witness: {other:?}"),
        }
    }
}

#[cfg(feature = "z3")]
pub use solver::solve_ledger_boundaries;
