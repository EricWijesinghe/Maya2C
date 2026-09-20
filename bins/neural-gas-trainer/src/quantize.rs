//! Float network → integer [`Model`].

use maya_fee_market::Model;
use maya_fee_market::model::{FEATURE_FRAC_BITS, HIDDEN, INPUTS, UNIT_GAIN_BPS, WEIGHT_FRAC_BITS};

use crate::dataset::Sample;
use crate::network::{Network, inputs};

fn weight(value: f64) -> i16 {
    let scaled = (value * f64::from(1u32 << WEIGHT_FRAC_BITS)).round();
    scaled.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
}

fn bias(value: f64) -> i32 {
    let scaled = (value * (1u64 << (WEIGHT_FRAC_BITS + FEATURE_FRAC_BITS)) as f64).round();
    scaled.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

/// Rounds every weight to Q12 and every bias to Q28.
#[must_use]
pub fn quantize(net: &Network) -> Model {
    let mut model = Model::ZERO;
    for unit in 0..HIDDEN {
        for input in 0..INPUTS {
            model.hidden_weights[unit][input] = weight(net.hidden_weights[unit][input]);
        }
        model.hidden_bias[unit] = bias(net.hidden_bias[unit]);
        model.output_weights[unit] = weight(net.output_weights[unit]);
    }
    model.output_bias = bias(net.output_bias);
    model
}

/// The largest gap between the float and integer gains, in basis points.
#[must_use]
pub fn max_disagreement_bps(net: &Network, model: &Model, samples: &[Sample]) -> u64 {
    samples
        .iter()
        .map(|sample| {
            let float = (net.gain(&inputs(sample)) * UNIT_GAIN_BPS as f64).round() as u64;
            float.abs_diff(model.gain(&sample.features))
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::generate;
    use crate::rng::SplitMix64;

    #[test]
    fn quantization_moves_the_gain_by_a_few_basis_points_at_most() {
        let samples = generate(31, 2_000);
        let mut rng = SplitMix64::new(8);
        let mut net = Network::new(&mut rng);
        net.train(&samples, 2, &mut rng);
        let model = quantize(&net);
        let error = max_disagreement_bps(&net, &model, &samples);
        assert!(error <= 20, "{error} bps");
    }
}
