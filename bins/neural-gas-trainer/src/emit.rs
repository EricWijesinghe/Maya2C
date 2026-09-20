//! Writing the integer model as Rust source.

use std::fmt::Write;

use maya_fee_market::Model;

use crate::evaluate::Metrics;
use crate::{EPOCHS, EVALUATION_SEED, TRAINING_BLOCKS, TRAINING_SEED, TrainingRun};

/// The generated `weights_v1.rs`.
///
/// Only integers and fixed decimal renderings of the report appear, so the text
/// is a pure function of the training run.
#[must_use]
pub fn source(run: &TrainingRun, linear: &Metrics, neural: &Metrics) -> String {
    let mut out = String::new();
    let report = [
        "//! The first trained network. **Generated — do not edit.**".to_owned(),
        "//!".to_owned(),
        "//! `cargo run --release -p maya-neural-gas-trainer` writes this file and".to_owned(),
        "//! `-- --check` fails if a fresh run differs. Trained on the synthetic".to_owned(),
        "//! simulator in `bins/neural-gas-trainer/src/simulator.rs`: every figure below".to_owned(),
        "//! is about that simulator, not about any real chain.".to_owned(),
        "//!".to_owned(),
        format!("//! - training seed `{TRAINING_SEED:#018x}`, {TRAINING_BLOCKS} blocks, {} labelled, {EPOCHS} epochs", run.samples),
        format!("//! - float gain MSE {:.6}; largest quantization error {} bps", run.float_mse, run.max_quantization_error_bps),
        format!("//! - evaluation seed `{EVALUATION_SEED:#018x}`, {} blocks, closed loop:", linear.blocks),
        "//!".to_owned(),
        "//! | Rule | mean \\|size − target\\| / target | mean \\|Δfee\\| / fee | blocks at cap | envelope violations |".to_owned(),
        "//! |---|---|---|---|---|".to_owned(),
        row("linear", linear),
        row("neural", neural),
    ];
    for line in report {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str("\nuse super::Model;\n\n/// The first trained network.\npub const MODEL_V1: Model = Model {\n");
    body(&mut out, &run.model);
    out.push_str("};\n");
    out
}

fn row(name: &str, m: &Metrics) -> String {
    format!(
        "//! | {name} | {:.4} | {:.4} | {:.2}% | {} |",
        m.mean_size_deviation,
        m.mean_fee_change,
        m.saturated_share * 100.0,
        m.envelope_violations
    )
}

fn body(out: &mut String, model: &Model) {
    out.push_str("    hidden_weights: [\n");
    for row in &model.hidden_weights {
        let cells: Vec<String> = row.iter().map(ToString::to_string).collect();
        let _ = writeln!(out, "        [{}],", cells.join(", "));
    }
    out.push_str("    ],\n");
    let list = |values: Vec<String>| values.join(", ");
    let _ = writeln!(
        out,
        "    hidden_bias: [{}],",
        list(model.hidden_bias.iter().map(ToString::to_string).collect())
    );
    let _ = writeln!(
        out,
        "    output_weights: [{}],",
        list(
            model
                .output_weights
                .iter()
                .map(ToString::to_string)
                .collect()
        )
    );
    let _ = writeln!(out, "    output_bias: {},", model.output_bias);
}
