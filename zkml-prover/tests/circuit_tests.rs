//! Soundness, one lie at a time.
//!
//! A circuit is only as good as what it refuses. Each test here hands the
//! circuit a witness that is honest everywhere except one field, and requires
//! `MockProver` to report the constraints unsatisfied. `MockProver` checks the
//! constraints directly, with no cryptography in the way, so a failure here is
//! a statement about the circuit and nothing else.
//!
//! The honest-witness tests matter as much: a circuit that refused everything
//! would pass every negative test in this file.

use std::path::PathBuf;

use halo2_axiom::dev::MockProver;
use maya_zkml::circuit::{MlpCircuit, Witness, public_inputs};
use maya_zkml::model::{MAX_CLASSES, MAX_HIDDEN, MAX_INPUTS, QuantizedMlp};
use maya_zkml::srs::K;
use maya_zkml_prover::onnx;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/zkml")
        .join(name)
}

fn model() -> QuantizedMlp {
    onnx::load(fixture("classifier.onnx")).expect("fixture loads")
}

/// The fixture's inputs, from the numpy reference file.
fn cases() -> Vec<(Vec<i64>, usize)> {
    let text = std::fs::read_to_string(fixture("classifier.expected.json")).expect("json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("parse");
    json["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|c| {
            let x = c["x"]
                .as_array()
                .expect("x")
                .iter()
                .map(|v| v.as_i64().expect("int"))
                .collect();
            (x, c["class"].as_u64().expect("class") as usize)
        })
        .collect()
}

fn satisfied(
    model: &QuantizedMlp,
    witness: Witness,
    public: Vec<halo2_axiom::halo2curves::bn256::Fr>,
) -> bool {
    let circuit = MlpCircuit::with_witness(model.clone(), witness);
    MockProver::run(K, &circuit, vec![public])
        .expect("the circuit fits in 2^K rows")
        .verify()
        .is_ok()
}

fn honest_is_satisfied(model: &QuantizedMlp, witness: Witness) -> bool {
    let public = witness.public_inputs();
    satisfied(model, witness, public)
}

#[test]
fn every_fixture_case_is_satisfied_by_its_honest_witness() {
    let model = model();
    for (x, class) in cases() {
        let witness = Witness::honest(&model, &x);
        assert_eq!(
            witness.class as usize, class,
            "circuit semantics disagree with numpy for {x:?}"
        );
        assert!(
            honest_is_satisfied(&model, witness),
            "honest witness refused for {x:?}"
        );
    }
}

#[test]
fn claiming_the_wrong_class_is_refused() {
    let model = model();
    for (x, class) in cases() {
        let mut witness = Witness::honest(&model, &x);
        let wrong = (class + 1) % model.classes();
        // A *consistent* lie: the one-hot vector agrees with the claimed class,
        // so only the argmax constraints stand in the way.
        witness.class = wrong as i64;
        witness.one_hot = (0..model.classes())
            .map(|k| i64::from(k == wrong))
            .collect();
        assert!(
            !honest_is_satisfied(&model, witness),
            "class {wrong} accepted for {x:?}"
        );
    }
}

#[test]
fn a_one_hot_vector_that_disagrees_with_the_class_is_refused() {
    let model = model();
    let (x, class) = cases().remove(0);
    let mut witness = Witness::honest(&model, &x);
    witness.one_hot = (0..model.classes())
        .map(|k| i64::from(k == (class + 1) % model.classes()))
        .collect();
    assert!(!honest_is_satisfied(&model, witness));
}

#[test]
fn two_bits_set_in_the_one_hot_vector_is_refused() {
    let model = model();
    let (x, _) = cases().remove(0);
    let mut witness = Witness::honest(&model, &x);
    witness.one_hot = vec![1; model.classes()];
    assert!(!honest_is_satisfied(&model, witness));
}

#[test]
fn a_non_boolean_one_hot_vector_is_refused() {
    // [2, -2, 1] sums to 1 and indexes to class 0 (0·2 + 1·(-2) + 2·1 = 0), so
    // the count and index constraints both hold. It selects 2·l0 − 2·l1 + l2 as
    // "the maximum", which a prover can make as large as it likes. Booleanity is
    // the only constraint that refuses it.
    let model = model();
    let (x, _) = cases()
        .into_iter()
        .find(|(_, class)| *class == 0)
        .expect("a class-0 case");
    let mut witness = Witness::honest(&model, &x);
    witness.one_hot = vec![2, -2, 1];
    assert!(!honest_is_satisfied(&model, witness));
}

/// Rebuilds everything downstream of the hidden layer from the given per-unit
/// decisions, so a lie about one unit stays *consistent*.
///
/// This is what an attacker would do, and it is what makes these tests mean
/// anything. Flip one ReLU bit and leave the rest honest, and some unrelated
/// constraint — the recomposition, the argmax — refuses the witness first; the
/// test passes whether or not the constraint it is named after exists. That is
/// not hypothetical: the first version of these tests passed with the ReLU sign
/// check deleted. A lie propagated forward can only be caught by the one
/// constraint that guards the decision it falsifies.
fn propagate(model: &QuantizedMlp, mut witness: Witness) -> Witness {
    let hidden: Vec<i64> = (0..model.hidden())
        .map(|j| {
            let q = witness.quotient[j];
            q + witness.saturated[j] * (127 - q)
        })
        .collect();
    let logits: Vec<i64> = (0..model.classes())
        .map(|k| {
            (0..model.hidden())
                .map(|j| hidden[j] * i64::from(model.w2(j, k)))
                .sum::<i64>()
                + i64::from(model.b2(k))
        })
        .collect();
    let mut class = 0;
    for (k, &logit) in logits.iter().enumerate() {
        if logit > logits[class] {
            class = k;
        }
    }
    witness.class = class as i64;
    witness.one_hot = (0..model.classes())
        .map(|k| i64::from(k == class))
        .collect();
    witness
}

/// Every fixture case's honest witness, with the first-layer accumulators.
fn honest_cases(model: &QuantizedMlp) -> Vec<(Witness, Vec<i64>)> {
    cases()
        .into_iter()
        .map(|(x, _)| {
            let acc = (0..model.hidden())
                .map(|j| {
                    (0..model.inputs())
                        .map(|i| x[i] * i64::from(model.w1(i, j)))
                        .sum::<i64>()
                        + i64::from(model.b1(j))
                })
                .collect();
            (Witness::honest(model, &x), acc)
        })
        .collect()
}

#[test]
fn claiming_relu_zeroed_a_positive_accumulator_is_refused() {
    // bit = 0 makes relu = 0, and q = rem = 0 recompose it perfectly; hidden
    // becomes 0 and everything after is recomputed. Only the sign check —
    // `−acc − 1` must be non-negative when the bit says "negative" — refuses.
    let model = model();
    let mut tried = 0;
    for (honest, acc) in honest_cases(&model) {
        for unit in (0..model.hidden()).filter(|&j| acc[j] > 0) {
            let mut lie = honest.clone();
            lie.relu_bit[unit] = 0;
            lie.quotient[unit] = 0;
            lie.remainder[unit] = 0;
            lie.saturated[unit] = 0;
            let lie = propagate(&model, lie);
            assert!(
                !honest_is_satisfied(&model, lie),
                "unit {unit}: positive acc zeroed"
            );
            tried += 1;
        }
    }
    assert!(
        tried > 0,
        "no fixture case had a positive accumulator to lie about"
    );
}

#[test]
fn claiming_an_unsaturated_unit_saturated_is_refused() {
    // c = 1 lifts hidden to 127 for a quotient below it. Downstream is
    // recomputed; only the saturation gap check refuses.
    let model = model();
    let mut tried = 0;
    for (honest, _) in honest_cases(&model) {
        for unit in (0..model.hidden()).filter(|&j| honest.quotient[j] < 127) {
            let mut lie = honest.clone();
            lie.saturated[unit] = 1;
            let lie = propagate(&model, lie);
            assert!(
                !honest_is_satisfied(&model, lie),
                "unit {unit}: false saturation accepted"
            );
            tried += 1;
        }
    }
    assert!(tried > 0);
}

#[test]
fn claiming_a_saturated_unit_unsaturated_is_refused() {
    // c = 0 lets hidden exceed 127. Only the saturation gap check refuses.
    let model = model();
    let mut tried = 0;
    for (honest, _) in honest_cases(&model) {
        for unit in (0..model.hidden()).filter(|&j| honest.quotient[j] >= 127) {
            let mut lie = honest.clone();
            lie.saturated[unit] = 0;
            let lie = propagate(&model, lie);
            assert!(
                !honest_is_satisfied(&model, lie),
                "unit {unit}: hidden above 127 accepted"
            );
            tried += 1;
        }
    }
    assert!(
        tried > 0,
        "the fixture's all-127 input must saturate some unit"
    );
}

#[test]
fn a_negative_remainder_is_refused() {
    // q + 1 and rem − 2^s recompose to the same value; the quotient (and so the
    // hidden unit) grows by one and everything after is recomputed. Only the
    // remainder's range check refuses.
    let model = model();
    let divisor = 1i64 << model.shift();
    for (honest, _) in honest_cases(&model) {
        for unit in 0..model.hidden() {
            let mut lie = honest.clone();
            lie.quotient[unit] += 1;
            lie.remainder[unit] -= divisor;
            let lie = propagate(&model, lie);
            assert!(
                !honest_is_satisfied(&model, lie),
                "unit {unit}: q+1 accepted"
            );
        }
    }
}

#[test]
fn a_remainder_at_or_above_the_divisor_is_refused() {
    // q − 1 and rem + 2^s: the remainder is non-negative but too large. Only the
    // headroom check, `2^s − 1 − rem ≥ 0`, refuses.
    let model = model();
    let divisor = 1i64 << model.shift();
    let mut tried = 0;
    for (honest, _) in honest_cases(&model) {
        for unit in (0..model.hidden()).filter(|&j| honest.quotient[j] > 0) {
            let mut lie = honest.clone();
            lie.quotient[unit] -= 1;
            lie.remainder[unit] += divisor;
            let lie = propagate(&model, lie);
            assert!(
                !honest_is_satisfied(&model, lie),
                "unit {unit}: q-1 accepted"
            );
            tried += 1;
        }
    }
    assert!(tried > 0);
}

/// A model whose accumulators and logits are exactly its biases: every weight
/// is zero. It exists so a test can put a unit at a chosen accumulator value
/// and lie about what happens to it.
fn biases_only(b1: Vec<i32>, b2: Vec<i32>, shift: u32) -> QuantizedMlp {
    let (inputs, hidden, classes) = (2, b1.len(), b2.len());
    QuantizedMlp::new(
        inputs,
        hidden,
        classes,
        vec![0; inputs * hidden],
        b1,
        vec![0; hidden * classes],
        b2,
        shift,
    )
    .expect("model")
}

#[test]
fn a_relu_bit_of_two_is_refused() {
    // bit = 2 doubles the unit: relu = 2·acc, and the sign check still passes
    // because t = 2·(2·acc) − acc + 2 − 1 = 3·acc + 1 is small and positive.
    // Quotient and remainder recompose 2·acc exactly. Only booleanity refuses.
    let model = biases_only(vec![50, 0], vec![0, 1], 0);
    let mut lie = Witness::honest(&model, &[0, 0]);
    assert_eq!(lie.quotient[0], 50);
    lie.relu_bit[0] = 2;
    lie.quotient[0] = 100;
    lie.remainder[0] = 0;
    lie.saturated[0] = 0;
    let lie = propagate(&model, lie);
    assert!(!honest_is_satisfied(&model, lie));
}

#[test]
fn a_quotient_that_does_not_recompose_the_accumulator_is_refused() {
    // q moves, the remainder stays honest, so both are in range and only
    // `q·2^s + rem = relu` connects the quotient to the accumulator at all.
    // Without it the hidden layer is whatever the prover likes.
    let model = model();
    for (honest, _) in honest_cases(&model) {
        for unit in (0..model.hidden()).filter(|&j| honest.quotient[j] < 100) {
            let mut lie = honest.clone();
            lie.quotient[unit] += 1;
            let lie = propagate(&model, lie);
            assert!(
                !honest_is_satisfied(&model, lie),
                "unit {unit}: free quotient accepted"
            );
        }
    }
}

#[test]
fn a_saturation_flag_of_minus_one_is_refused() {
    // q = 100 is below the ceiling. c = −1 makes hidden = q − (127 − q) = 73,
    // and the gap check still passes: c·(2q − 253) − q + 126 = 53 − 100 + 126
    // = 79. Only booleanity refuses.
    let model = biases_only(vec![100, 0], vec![0, 1], 0);
    let mut lie = Witness::honest(&model, &[0, 0]);
    assert_eq!((lie.quotient[0], lie.saturated[0]), (100, 0));
    lie.saturated[0] = -1;
    let lie = propagate(&model, lie);
    assert!(!honest_is_satisfied(&model, lie));
}

#[test]
fn an_empty_one_hot_vector_is_refused() {
    // Every logit negative; the true class is 1. An all-zero one-hot indexes to
    // 0, selects 0 as "the maximum", and every slack 0 − logit_j is positive. So
    // class 0 passes the index and argmax checks. Only "exactly one bit set"
    // refuses.
    let model = biases_only(vec![0, 0], vec![-10, -5, -20], 0);
    let honest = Witness::honest(&model, &[0, 0]);
    assert_eq!(honest.class, 1);
    let mut lie = honest;
    lie.class = 0;
    lie.one_hot = vec![0, 0, 0];
    assert!(!honest_is_satisfied(&model, lie));
}

#[test]
fn a_claimed_class_disconnected_from_the_one_hot_vector_is_refused() {
    // The one-hot vector is honest, so the argmax checks pass; the claimed —
    // public — class is not the one it encodes. Only the index constraint ties
    // the two together.
    let model = model();
    for (x, class) in cases() {
        let mut lie = Witness::honest(&model, &x);
        lie.class = ((class + 1) % model.classes()) as i64;
        assert!(
            !honest_is_satisfied(&model, lie),
            "class detached from one-hot for {x:?}"
        );
    }
}

#[test]
fn an_input_outside_int8_is_refused_even_with_a_consistent_witness() {
    // Every other witness value is computed honestly *for* the out-of-range
    // input, so the model arithmetic all checks out. Only the input range check
    // can refuse this, and it must.
    let model = model();
    for bad in [128, -129, 200, 1 << 20] {
        let mut x = vec![0i64; model.inputs()];
        x[0] = bad;
        let witness = Witness::honest(&model, &x);
        assert!(
            !honest_is_satisfied(&model, witness),
            "input {bad} accepted"
        );
    }
}

#[test]
fn the_input_extremes_are_accepted() {
    let model = model();
    for edge in [-128, 127] {
        let witness = Witness::honest(&model, &vec![edge; model.inputs()]);
        assert!(honest_is_satisfied(&model, witness), "input {edge} refused");
    }
}

#[test]
fn a_proof_is_bound_to_its_public_inputs() {
    // An honest witness, checked against a different claimed class or input.
    // The constraints hold internally; only the instance binding fails.
    let model = model();
    let (x, class) = cases().remove(0);
    let witness = Witness::honest(&model, &x);

    let wrong_class = public_inputs(&x, ((class + 1) % model.classes()) as i64);
    assert!(!satisfied(&model, witness.clone(), wrong_class));

    let mut other_x = x.clone();
    other_x[0] = if other_x[0] == 0 { 1 } else { 0 };
    let wrong_input = public_inputs(&other_x, class as i64);
    assert!(!satisfied(&model, witness, wrong_input));
}

/// A model where two logits tie, to pin "the first maximum wins".
fn tied_model() -> QuantizedMlp {
    let (inputs, hidden, classes) = (2, 2, 3);
    QuantizedMlp::new(
        inputs,
        hidden,
        classes,
        vec![0; inputs * hidden],
        vec![0; hidden],
        // No dependence on the input at all: logits are exactly the biases.
        vec![0; hidden * classes],
        vec![5, 9, 9],
        0,
    )
    .expect("model")
}

#[test]
fn a_tie_goes_to_the_first_maximum_and_only_the_first() {
    let model = tied_model();
    let honest = Witness::honest(&model, &[0, 0]);
    assert_eq!(
        honest.class, 1,
        "logits [5, 9, 9]: the first 9 is at index 1"
    );
    assert!(honest_is_satisfied(&model, honest.clone()));

    // Index 2 is *also* a maximum. Claiming it is a lie about "first".
    let mut last = honest;
    last.class = 2;
    last.one_hot = vec![0, 0, 1];
    assert!(!honest_is_satisfied(&model, last));
}

#[test]
fn the_largest_model_the_bounds_allow_fits_the_circuit() {
    // The bounds in `model.rs` promise this shape proves. If the layout grew
    // past 2^K rows, `MockProver::run` would say so here rather than in
    // somebody's deployment.
    let (inputs, hidden, classes) = (MAX_INPUTS, MAX_HIDDEN, MAX_CLASSES);
    let model = QuantizedMlp::new(
        inputs,
        hidden,
        classes,
        (0..inputs * hidden).map(|i| (i % 255) as i8).collect(),
        vec![-4096; hidden],
        (0..hidden * classes)
            .map(|i| ((i * 7) % 255) as i8)
            .collect(),
        vec![4096; classes],
        3,
    )
    .expect("model at the bounds");
    let witness = Witness::honest(&model, &vec![127; inputs]);
    assert!(honest_is_satisfied(&model, witness));
}
