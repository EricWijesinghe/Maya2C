//! Cost of accepting one finality checkpoint at a 7-member and a 100-member
//! committee (ADR-089, milestone M0).
//!
//! Each iteration runs the client's real path: the header-id binding, the trusted
//! period, and the node's quorum verifier on n − f ML-DSA-87 signatures. The
//! committee has n members and the checkpoint carries exactly the quorum, so the
//! verifier checks the smallest set it accepts.
//!
//! `cargo bench -p maya-light-client --bench finality`

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{Criterion, criterion_group, criterion_main};
use custom_l1_node::consensus::bft::attest::{Attestation, Checkpoint};
use custom_l1_node::core::{BlockHeader, ChainTag};
use custom_l1_node::crypto::validator_keys::{SigningKey, signing_key_from_seed};
use maya_light_client::{FinalityClient, TrustAnchor, TrustPolicy};
use std::collections::BTreeMap;

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

/// The quorum n − f for a committee of `n`, with f = floor((n − 1) / 3).
fn quorum(n: usize) -> usize {
    n - (n - 1) / 3
}

fn bench_accept(c: &mut Criterion) {
    let mut group = c.benchmark_group("finality_accept_checkpoint");
    for n in [7usize, 100] {
        let signing: Vec<SigningKey> = (1..=n)
            .map(|i| signing_key_from_seed(&[u8::try_from(i).unwrap(); 32]).unwrap())
            .collect();
        let committee = signing.iter().map(SigningKey::verifying_key).collect();
        let anchor = TrustAnchor {
            epoch: 0,
            height: 0,
            block: [0; 32],
            committee,
            weights: None,
        };
        let policy = TrustPolicy::new(1_000, 10_000).unwrap();
        let header = BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [1; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: [0xFF; 32],
            tx_root: [0; 32],
        };
        let block = header.id();
        let signatures: BTreeMap<u16, Vec<u8>> = (0..quorum(n))
            .map(|v| {
                let v = u16::try_from(v).unwrap();
                let att =
                    Attestation::sign(&signing[usize::from(v)], v, &CHAIN, 0, 10, block).unwrap();
                (v, att.signature)
            })
            .collect();
        let checkpoint = Checkpoint {
            epoch: 0,
            height: 10,
            block,
            signatures,
        };
        let mut client = FinalityClient::new(CHAIN, policy, anchor).unwrap();

        group.bench_function(format!("n{n}_quorum{}", quorum(n)), |b| {
            b.iter(|| {
                client
                    .accept_checkpoint(
                        std::hint::black_box(&checkpoint),
                        std::hint::black_box(&header),
                        10,
                    )
                    .unwrap();
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_accept);
criterion_main!(benches);
