//! Ten nodes fine-tune a model together without revealing their updates or
//! their data.
//!
//! Everything runs in one process, with messages passed as values. The claims
//! checked are the ones a simulation can check:
//!
//! - the aggregator's output is exactly the sum of the noised updates, with
//!   every mask removed — including the masks of nodes that dropped out;
//! - a single masked input looks nothing like the update inside it, and no
//!   byte of any node's local samples appears in anything it sends;
//! - the round refuses to unmask below its threshold, refuses a tampered share,
//!   and a node will not answer for two different survivor sets;
//! - a model trained this way reaches useful accuracy, and the privacy spent
//!   is reported as an (ε, δ) bound rather than implied.
//!
//! What a simulation cannot check — side channels, a malicious aggregator
//! lying about survivors, enclave attestation — is stated in
//! `docs/confidential-ai.md`, not asserted here.

use maya_confidential_ai::dp::{Accountant, discrete_gaussian, sigma_sq_for};
use maya_confidential_ai::protocol::{
    Advertisement, Aggregator, KeyEnvelope, Participant, RoundConfig, ShareEnvelope,
};
use maya_confidential_ai::quantize::{Quantizer, to_residues};
use maya_confidential_ai::random::Randomness;
use maya_confidential_ai::{Error, Result};

const NODES: usize = 10;
const THRESHOLD: usize = 6;
const DIMENSION: usize = 8;
/// Per-coordinate input bound: covers a clipped, scaled update plus 12σ of
/// noise in the training test, and fits ten nodes inside 2^31.
const MAX_INPUT: u64 = 1 << 24;

/// Runs steps 1–3 of a round and returns its participants and aggregator.
fn setup(round: u64, dimension: usize) -> (Vec<Participant>, Aggregator) {
    let config = RoundConfig::new(round, NODES, THRESHOLD, dimension, MAX_INPUT).expect("config");
    let mut participants: Vec<Participant> = (0..NODES as u16)
        .map(|index| {
            let mut entropy = [0u8; 32];
            entropy[..8].copy_from_slice(&round.to_le_bytes());
            entropy[8..10].copy_from_slice(&index.to_le_bytes());
            Participant::new(config, index, &entropy).expect("participant")
        })
        .collect();
    let advertisements: Vec<Advertisement> =
        participants.iter().map(Participant::advertise).collect();
    let keys: Vec<KeyEnvelope> = participants
        .iter_mut()
        .flat_map(|p| p.encapsulate(&advertisements).expect("encapsulate"))
        .collect();
    for participant in &mut participants {
        participant.accept_keys(&keys).expect("accept");
    }
    let shares: Vec<ShareEnvelope> = participants
        .iter_mut()
        .flat_map(|p| p.share_seed().expect("share"))
        .collect();
    for participant in &mut participants {
        participant.receive_shares(&shares).expect("receive");
    }
    let aggregator = Aggregator::new(config, &advertisements).expect("aggregator");
    (participants, aggregator)
}

/// Submits the updates of `present` nodes and unmasks.
fn aggregate(
    participants: &mut [Participant],
    aggregator: &mut Aggregator,
    updates: &[Vec<i64>],
    present: &[u16],
) -> Result<Vec<u32>> {
    for &index in present {
        let input = participants[usize::from(index)].masked_input(&updates[usize::from(index)])?;
        aggregator.submit(input)?;
    }
    let survivors = aggregator.survivors();
    let responses = present
        .iter()
        .map(|&index| participants[usize::from(index)].unmask_response(&survivors))
        .collect::<Result<Vec<_>>>()?;
    aggregator.unmask(&responses)
}

fn plain_sum(updates: &[Vec<i64>], present: &[u16]) -> Vec<u32> {
    let mut sum = vec![0i64; updates[0].len()];
    for &index in present {
        for (total, value) in sum.iter_mut().zip(&updates[usize::from(index)]) {
            *total += value;
        }
    }
    to_residues(&sum)
}

fn random_updates(seed: u8) -> Vec<Vec<i64>> {
    let mut rng = Randomness::from_seed(&[seed; 32], "test updates");
    (0..NODES)
        .map(|_| {
            (0..DIMENSION)
                .map(|_| rng.below(200_001) as i64 - 100_000)
                .collect()
        })
        .collect()
}

#[test]
fn masked_aggregation_returns_exactly_the_sum_of_the_updates() {
    let (mut participants, mut aggregator) = setup(1, DIMENSION);
    let updates = random_updates(1);
    let everyone: Vec<u16> = (0..NODES as u16).collect();
    let sum = aggregate(&mut participants, &mut aggregator, &updates, &everyone).expect("round");
    assert_eq!(sum, plain_sum(&updates, &everyone));
}

#[test]
fn two_dropped_nodes_are_unmasked_out_of_the_sum() {
    let (mut participants, mut aggregator) = setup(2, DIMENSION);
    let updates = random_updates(2);
    let survivors: Vec<u16> = (0..NODES as u16).filter(|i| *i != 3 && *i != 8).collect();
    let sum = aggregate(&mut participants, &mut aggregator, &updates, &survivors).expect("round");
    assert_eq!(sum, plain_sum(&updates, &survivors));
}

#[test]
fn below_the_threshold_the_round_refuses_to_unmask() {
    let (mut participants, mut aggregator) = setup(3, DIMENSION);
    let updates = random_updates(3);
    let few: Vec<u16> = (0..(THRESHOLD - 1) as u16).collect();
    assert!(matches!(
        aggregate(&mut participants, &mut aggregator, &updates, &few),
        Err(Error::BelowThreshold { .. })
    ));
}

#[test]
fn a_node_never_answers_for_two_different_survivor_sets() {
    let (mut participants, _) = setup(4, DIMENSION);
    let all: Vec<u16> = (0..NODES as u16).collect();
    let fewer: Vec<u16> = (0..(NODES - 1) as u16).collect();
    participants[0]
        .unmask_response(&fewer)
        .expect("first answer");
    assert!(participants[0].unmask_response(&all).is_err());
}

#[test]
fn a_tampered_seed_share_is_caught_by_the_commitment() {
    let (mut participants, mut aggregator) = setup(5, DIMENSION);
    let updates = random_updates(5);
    for (index, participant) in participants.iter().enumerate() {
        aggregator
            .submit(participant.masked_input(&updates[index]).expect("mask"))
            .expect("submit");
    }
    let survivors = aggregator.survivors();
    let mut responses: Vec<_> = participants
        .iter_mut()
        .map(|p| p.unmask_response(&survivors).expect("answer"))
        .collect();
    for response in &mut responses {
        if let Some(share) = response.self_seed_shares.get_mut(&0) {
            share.y[0] ^= 1;
        }
    }
    assert_eq!(
        aggregator.unmask(&responses),
        Err(Error::CommitmentMismatch(0))
    );
}

#[test]
fn a_masked_input_hides_its_update_and_no_sample_bytes_leave_a_node() {
    let (participants, _) = setup(6, 64);
    let update = vec![0i64; 64];
    let masked = participants[0].masked_input(&update).expect("mask");
    // A zero update masked: every coordinate uniform, none still zero.
    assert!(masked.values.iter().filter(|&&v| v == 0).count() <= 1);
    let mean = masked.values.iter().map(|&v| f64::from(v)).sum::<f64>() / 64.0;
    assert!((mean / f64::from(u32::MAX) - 0.5).abs() < 0.15, "{mean}");

    // A node's samples never appear in what it sends: search the masked input
    // for every 8-byte window of the node's local feature bytes.
    let samples = local_dataset(0, 16).0;
    let sent: Vec<u8> = masked.values.iter().flat_map(|v| v.to_le_bytes()).collect();
    for sample in samples.iter().flatten() {
        let bytes = sample.to_le_bytes();
        assert!(!sent.windows(8).any(|window| window == bytes));
    }
}

// ---------------------------------------------------------------------------
// Federated fine-tuning
// ---------------------------------------------------------------------------

/// The model every node's data is labelled by.
const TRUE_WEIGHTS: [f64; DIMENSION] = [1.5, -2.0, 0.5, 3.0, -1.0, 0.0, 2.5, -0.5];

/// `count` samples in [-1, 1]^d with labels from [`TRUE_WEIGHTS`].
fn local_dataset(node: u16, count: usize) -> (Vec<[f64; DIMENSION]>, Vec<f64>) {
    let mut seed = [0u8; 32];
    seed[..2].copy_from_slice(&node.to_le_bytes());
    let mut rng = Randomness::from_seed(&seed, "local dataset");
    let features: Vec<[f64; DIMENSION]> = (0..count)
        .map(|_| std::array::from_fn(|_| rng.unit_f64() * 2.0 - 1.0))
        .collect();
    let labels = features
        .iter()
        .map(|x| f64::from(u8::from(dot(&TRUE_WEIGHTS, x) > 0.0)))
        .collect();
    (features, labels)
}

fn dot(a: &[f64; DIMENSION], b: &[f64; DIMENSION]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn sigmoid(z: f64) -> f64 {
    1.0 / (1.0 + (-z).exp())
}

/// Mean logistic-loss gradient over a node's data.
fn gradient(weights: &[f64; DIMENSION], data: &(Vec<[f64; DIMENSION]>, Vec<f64>)) -> Vec<f64> {
    let mut grad = [0.0; DIMENSION];
    for (x, y) in data.0.iter().zip(&data.1) {
        let error = sigmoid(dot(weights, x)) - y;
        for (g, xi) in grad.iter_mut().zip(x) {
            *g += error * xi;
        }
    }
    grad.iter().map(|g| g / data.0.len() as f64).collect()
}

fn accuracy(weights: &[f64; DIMENSION], data: &(Vec<[f64; DIMENSION]>, Vec<f64>)) -> f64 {
    let correct = data
        .0
        .iter()
        .zip(&data.1)
        .filter(|(x, y)| f64::from(u8::from(dot(weights, x) > 0.0)) == **y)
        .count();
    correct as f64 / data.0.len() as f64
}

#[test]
fn ten_nodes_fine_tune_a_model_privately_and_report_the_privacy_spent() {
    const ROUNDS: u64 = 30;
    const RHO_PER_ROUND: f64 = 4.0;
    const LEARNING_RATE: f64 = 2.0;

    let quantizer = Quantizer::new(1.0, 65_536.0).expect("quantizer");
    let sensitivity = quantizer.sensitivity(DIMENSION);
    let sigma_sq = sigma_sq_for(sensitivity, RHO_PER_ROUND).expect("sigma");
    let noise_bound = 12 * sigma_sq.isqrt();
    quantizer
        .check_headroom(NODES, noise_bound)
        .expect("headroom");

    let datasets: Vec<_> = (0..NODES as u16)
        .map(|node| local_dataset(node, 200))
        .collect();
    let held_out = local_dataset(999, 2_000);
    let mut weights = [0.0; DIMENSION];
    let mut accountant = Accountant::default();
    let everyone: Vec<u16> = (0..NODES as u16).collect();
    let initial = accuracy(&weights, &held_out);

    for round in 0..ROUNDS {
        let mut rng = Randomness::from_seed(&[round as u8; 32], "training noise");
        let updates: Vec<Vec<i64>> = datasets
            .iter()
            .map(|data| {
                let mut encoded = quantizer
                    .encode(&gradient(&weights, data), &mut rng)
                    .expect("encode");
                for value in &mut encoded {
                    *value += discrete_gaussian(&mut rng, sigma_sq).expect("noise");
                }
                encoded
            })
            .collect();

        let (mut participants, mut aggregator) = setup(1_000 + round, DIMENSION);
        let sum =
            aggregate(&mut participants, &mut aggregator, &updates, &everyone).expect("round");
        assert_eq!(
            sum,
            plain_sum(&updates, &everyone),
            "round {round} lost a mask"
        );

        for (weight, step) in weights.iter_mut().zip(quantizer.decode_mean(&sum, NODES)) {
            *weight -= LEARNING_RATE * step;
        }
        accountant.record(sensitivity, sigma_sq).expect("record");
    }

    let trained = accuracy(&weights, &held_out);
    assert!(trained >= 0.85, "accuracy {trained} (from {initial})");
    assert!((accountant.rho() - ROUNDS as f64 * RHO_PER_ROUND).abs() < 0.5);

    // The honest reading: with ten participants and conservative per-node
    // noise, the bound after thirty rounds is weak. The accountant says so
    // instead of the test implying otherwise.
    let epsilon = accountant.epsilon(1e-5);
    assert!(epsilon.is_finite() && epsilon > 100.0, "epsilon {epsilon}");
}
