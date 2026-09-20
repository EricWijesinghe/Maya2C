//! `neural-gas-trainer`: regenerate, or check, the committed weight file.
//!
//! ```text
//! cargo run --release -p maya-neural-gas-trainer            # write
//! cargo run --release -p maya-neural-gas-trainer -- --check # fail on drift
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use maya_neural_gas_trainer::evaluate::{Controller, run};
use maya_neural_gas_trainer::{EVALUATION_BLOCKS, EVALUATION_SEED, emit, train};

fn weights_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fee-market/src/model/weights_v1.rs")
}

fn main() -> ExitCode {
    let check = std::env::args().skip(1).any(|arg| arg == "--check");

    let training = train();
    let linear = run(EVALUATION_SEED, EVALUATION_BLOCKS, Controller::Linear);
    let neural = run(
        EVALUATION_SEED,
        EVALUATION_BLOCKS,
        Controller::Neural(&training.model),
    );
    let source = emit::source(&training, &linear, &neural);

    eprintln!(
        "float MSE {:.6}, quantization error {} bps",
        training.float_mse, training.max_quantization_error_bps
    );
    eprintln!("linear: {linear:?}");
    eprintln!("neural: {neural:?}");

    let path = weights_path();
    if check {
        return match std::fs::read_to_string(&path) {
            Ok(committed) if committed == source => {
                eprintln!("{} matches a fresh run", path.display());
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!("{} differs from a fresh run: regenerate it", path.display());
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("reading {}: {error}", path.display());
                ExitCode::FAILURE
            }
        };
    }
    match std::fs::write(&path, source) {
        Ok(()) => {
            eprintln!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("writing {}: {error}", path.display());
            ExitCode::FAILURE
        }
    }
}
