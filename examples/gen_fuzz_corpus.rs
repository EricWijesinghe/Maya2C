//! Writes the seed corpus for every fuzz target under `fuzz/corpus/`.
//!
//! ```text
//! cargo run --example gen_fuzz_corpus
//! ```
//!
//! # Why the seeds are generated rather than checked in by hand
//!
//! A hand-rolled seed is a second, undocumented copy of the wire format. It
//! rots the moment a field moves, and it rots *silently*: a stale seed simply
//! stops decoding, and the fuzzer carries on against a corpus that no longer
//! reaches the code it was written for. Generating them through the real
//! encoders means a format change either updates the corpus or fails to
//! compile.
//!
//! # Why this lives here and not in `fuzz/`
//!
//! The fuzz crate depends on `libfuzzer-sys`, which builds libFuzzer's C++ and
//! does not support `x86_64-pc-windows-msvc`. Corpus generation has nothing to
//! do with libFuzzer, and putting it there would make the corpus regenerable
//! only on the platforms that can fuzz. As an example in the node's own crate
//! it builds anywhere the node does.
//!
//! # Determinism
//!
//! No key generation and no signing. Signature bytes are fixed fillers, which
//! is correct for a decode corpus: no target calls `verify`, and a real ML-DSA
//! signature costs ~105 ms to produce while teaching the decoder nothing a
//! filler does not. Determinism is what makes the corpus reviewable in a diff —
//! regenerating it either changes nothing or shows exactly what moved.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::payload::ShieldedJoinSplit;
use custom_l1_node::core::{
    ChannelClosure, ChannelOpen, ContractCall, ContractDeploy, RevocationProof, TxKind,
};
use custom_l1_node::crypto::SLH_DSA_SIGNATURE_LENGTH;
use custom_l1_node::crypto::hybrid::{HybridPublicKey, HybridSignature};
use custom_l1_node::crypto::keys::SIGNATURE_LENGTH as ML_DSA_SIGNATURE_LENGTH;
use custom_l1_node::{Account, Block, BlockHeader, Transaction, TxInput, TxOutput};

/// Root of the corpus tree, relative to this crate's manifest directory.
///
/// Anchored to `CARGO_MANIFEST_DIR` rather than the working directory, so the
/// corpus lands in the same place whether this is run from the repository root
/// or anywhere below it.
fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz")
        .join("corpus")
}

fn main() -> io::Result<()> {
    let mut written = 0usize;

    for (name, bytes) in transaction_seeds() {
        write_seed("tx_decode", &name, &bytes)?;
        written += 1;
    }
    for (name, bytes) in header_seeds() {
        write_seed("header_decode", &name, &bytes)?;
        written += 1;
    }
    for (name, bytes) in block_seeds() {
        write_seed("block_decode", &name, &bytes)?;
        written += 1;
    }
    for (name, bytes) in payload_seeds() {
        write_seed("payload_decode", &name, &bytes)?;
        written += 1;
    }
    for (name, bytes) in account_seeds() {
        write_seed("account_decode", &name, &bytes)?;
        written += 1;
    }
    for (name, bytes) in sv2_frame_seeds() {
        write_seed("sv2_frame_decode", &name, &bytes)?;
        written += 1;
    }

    println!("wrote {written} seeds under {}", corpus_root().display());
    Ok(())
}

/// Writes one seed, creating the target's corpus directory if needed.
fn write_seed(target: &str, name: &str, bytes: &[u8]) -> io::Result<()> {
    let dir = corpus_root().join(target);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join(name), bytes)
}

/// A filler public key that is structurally valid and controls nothing.
///
/// `HybridPublicKey::default` is all zeros and does decode — every 1952-byte
/// string is some ML-DSA coefficient vector — but an all-zero seed gives the
/// mutator nothing to work with in 1984 bytes of key material. A byte pattern
/// keyed on the seed index gives it structure to break.
fn filler_public_key(tag: u8) -> Box<HybridPublicKey> {
    let mut key = HybridPublicKey::default();
    key.lattice.fill(tag);
    key.hash_based.fill(tag.wrapping_add(1));
    Box::new(key)
}

/// A filler signature pair. Never verified by any target — see the module docs.
fn filler_signature(tag: u8) -> Box<HybridSignature> {
    Box::new(HybridSignature {
        lattice: [tag; ML_DSA_SIGNATURE_LENGTH],
        hash_based: [tag.wrapping_add(1); SLH_DSA_SIGNATURE_LENGTH],
    })
}

/// A closure with both key pairs and both signature pairs populated.
///
/// This is the largest structure the codec handles — roughly 25.8 KB — and the
/// only one carrying four proofs, so it is the seed that exercises
/// `CLOSURE_SIZE` accounting in `read_collection_len`.
fn filler_closure(seq: u64, balance_a: u64, balance_b: u64) -> ChannelClosure {
    ChannelClosure {
        channel_id: [0x11; 32],
        seq,
        balance_a,
        balance_b,
        revocation_commitment: [0x22; 32],
        pubkey_a: filler_public_key(0x30),
        pubkey_b: filler_public_key(0x40),
        sig_a: filler_signature(0x50),
        sig_b: filler_signature(0x60),
    }
}

/// A transaction with a filler signature attached, so the corpus covers the
/// presence flag's `1` branch as well as its `0` branch.
fn signed(mut transaction: Transaction) -> Transaction {
    transaction.public_key = filler_public_key(0x70);
    transaction.signature = Some(filler_signature(0x80));
    transaction
}

fn transaction_seeds() -> Vec<(String, Vec<u8>)> {
    let outputs = vec![
        TxOutput {
            amount: 1,
            recipient: [0xAA; 32],
        },
        // A near-ceiling amount, so the corpus already contains the operand
        // that makes `total_outputs` overflow after one mutation.
        TxOutput {
            amount: u64::MAX,
            recipient: [0xBB; 32],
        },
    ];
    let inputs = vec![TxInput {
        prev_tx: [0xCC; 32],
        index: u32::MAX,
    }];

    let transactions = vec![
        ("unsigned_empty", Transaction::new(vec![], vec![], 0)),
        (
            "unsigned_transfer",
            Transaction::new(inputs.clone(), outputs.clone(), 7),
        ),
        (
            "signed_transfer",
            signed(Transaction::new(inputs, outputs, u64::MAX)),
        ),
        (
            "signed_open_channel",
            signed(Transaction::with_kind(
                TxKind::OpenChannel(ChannelOpen {
                    counterparty: [0xDD; 32],
                    funding: 1_000,
                    dispute_window: 144,
                }),
                1,
            )),
        ),
        (
            "signed_deploy_contract",
            signed(Transaction::with_kind(
                TxKind::DeployContract(ContractDeploy {
                    code: b"\0asm\x01\0\0\0".to_vec(),
                }),
                2,
            )),
        ),
        (
            "signed_settle_batch",
            signed(Transaction::with_kind(
                TxKind::SettleBatch(vec![filler_closure(1, 60, 40)]),
                3,
            )),
        ),
    ];

    transactions
        .into_iter()
        .map(|(name, transaction)| (name.to_string(), transaction.to_bytes()))
        .collect()
}

fn header_seeds() -> Vec<(String, Vec<u8>)> {
    let headers = vec![
        (
            "genesis_shaped",
            BlockHeader {
                prev_hash: [0; 32],
                state_root: [0; 32],
                timestamp: 0,
                nonce: 0,
                difficulty_target: [0xFF; 32],
            },
        ),
        (
            "saturated",
            BlockHeader {
                prev_hash: [0xFF; 32],
                state_root: [0xFF; 32],
                timestamp: u64::MAX,
                nonce: u64::MAX,
                difficulty_target: [0; 32],
            },
        ),
        (
            "typical",
            BlockHeader {
                prev_hash: [0x01; 32],
                state_root: [0x02; 32],
                timestamp: 1_700_000_000,
                nonce: 42,
                // Leading zero bytes then all-ones: the shape a real retargeted
                // target has, which an all-`0xFF` or all-zero seed does not.
                difficulty_target: {
                    let mut target = [0xFFu8; 32];
                    target[0] = 0x00;
                    target[1] = 0x00;
                    target[2] = 0x0F;
                    target
                },
            },
        ),
    ];

    headers
        .into_iter()
        .map(|(name, header)| (name.to_string(), header.serialize().to_vec()))
        .collect()
}

fn block_seeds() -> Vec<(String, Vec<u8>)> {
    let header = BlockHeader {
        prev_hash: [0x01; 32],
        state_root: [0x02; 32],
        timestamp: 1_700_000_000,
        nonce: 42,
        difficulty_target: [0xFF; 32],
    };

    let one = signed(Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 500,
            recipient: [0xAA; 32],
        }],
        0,
    ));
    let two = signed(Transaction::new(
        vec![TxInput {
            prev_tx: [0xCC; 32],
            index: 0,
        }],
        vec![TxOutput {
            amount: u64::MAX,
            recipient: [0xBB; 32],
        }],
        1,
    ));

    let blocks = vec![
        ("empty", Block::new(header.clone(), vec![])),
        ("one_tx", Block::new(header.clone(), vec![one.clone()])),
        ("two_tx", Block::new(header, vec![one, two])),
    ];

    blocks
        .into_iter()
        .map(|(name, block)| (name.to_string(), block.to_bytes()))
        .collect()
}

fn payload_seeds() -> Vec<(String, Vec<u8>)> {
    let joinsplit = ShieldedJoinSplit {
        anchor: [0x01; 32],
        nullifiers: [[0x02; 32], [0x03; 32]],
        commitments: [[0x04; 32], [0x05; 32]],
        // The triple that `ShieldedPool::settle` must not wrap on. Seeding it
        // adjacent to the ceiling means one mutation reaches the overflow.
        public_in: u64::MAX,
        public_out: u64::MAX - 1,
        fee: 1,
        recipient: [0x06; 32],
        proof: [0x07; 192],
    };

    let kinds = vec![
        (
            "open_channel",
            TxKind::OpenChannel(ChannelOpen {
                counterparty: [0xDD; 32],
                funding: u64::MAX,
                dispute_window: 144,
            }),
        ),
        (
            "cooperative_close",
            TxKind::CooperativeClose(filler_closure(1, u64::MAX, 1)),
        ),
        (
            "dispute_close",
            TxKind::DisputeClose(filler_closure(2, 60, 40)),
        ),
        (
            "penalty_claim",
            TxKind::PenaltyClaim(RevocationProof {
                channel_id: [0x11; 32],
                revoked_seq: 1,
                secret: [0x99; 32],
            }),
        ),
        ("settle_batch_empty", TxKind::SettleBatch(vec![])),
        (
            "settle_batch_two",
            TxKind::SettleBatch(vec![filler_closure(1, 60, 40), filler_closure(2, 10, 90)]),
        ),
        ("finalize_dispute", TxKind::FinalizeDispute([0x11; 32])),
        (
            "deploy_contract",
            TxKind::DeployContract(ContractDeploy {
                code: b"\0asm\x01\0\0\0".to_vec(),
            }),
        ),
        (
            "call_contract",
            TxKind::CallContract(ContractCall {
                contract: [0xEE; 32],
                input: vec![0xAB; 64],
                gas_limit: u64::MAX,
            }),
        ),
        ("shielded", TxKind::Shielded(Box::new(joinsplit))),
    ];

    kinds
        .into_iter()
        .map(|(name, kind)| {
            let mut buf = Vec::new();
            kind.encode_into(&mut buf);

            // Cheap self-check: a seed that does not decode is a seed the
            // fuzzer will discard, and it would do so silently.
            let mut reader = ByteReader::new(&buf);
            TxKind::decode(&mut reader).expect("generated payload seed must decode");
            reader
                .finish()
                .expect("generated payload seed must be exact");

            (name.to_string(), buf)
        })
        .collect()
}

fn account_seeds() -> Vec<(String, Vec<u8>)> {
    let accounts = vec![
        ("zero", Account::default()),
        (
            "saturated",
            Account {
                balance: u64::MAX,
                nonce: u64::MAX,
            },
        ),
        (
            "typical",
            Account {
                balance: 1_000_000,
                nonce: 3,
            },
        ),
    ];

    accounts
        .into_iter()
        .map(|(name, account)| (name.to_string(), account.encode().to_vec()))
        .collect()
}

/// Seeds for `sv2_frame_decode`: the pool protocol's front door.
///
/// Generated through `Message::to_frame` for the same reason as every other
/// seed here — a hand-written frame is a second copy of the wire format that
/// stops decoding the moment a field moves, silently.
///
/// The set deliberately spans the shapes the mutator would otherwise have to
/// discover: a message with no payload beyond its fixed fields, one with a
/// string field the mutator can lengthen past its declared size, the
/// share submission that carries the pool's whole load, and a frame whose
/// `channel_msg` flag is the thing worth flipping.
fn sv2_frame_seeds() -> Vec<(String, Vec<u8>)> {
    use maya_stratum_v2::Message;
    use maya_stratum_v2::messages::{
        CloseChannel, NewMiningJob, OpenStandardMiningChannel, OpenStandardMiningChannelSuccess,
        Protocol, SetNewPrevHash, SetTarget, SetupConnection, SubmitSharesStandard,
        SubmitWorkerTelemetry,
    };

    let messages: Vec<(&str, Message)> = vec![
        (
            "setup_connection",
            Message::SetupConnection(SetupConnection {
                protocol: Protocol::Mining,
                min_version: 2,
                max_version: 2,
                flags: 0,
                endpoint_host: "pool.maya.example".to_string(),
                endpoint_port: 3333,
                vendor: "Maya".to_string(),
                hardware_version: "rev-2".to_string(),
                firmware: "0.1.0".to_string(),
                device_id: "farm-07.rig-3".to_string(),
            }),
        ),
        (
            "open_channel",
            Message::OpenStandardMiningChannel(OpenStandardMiningChannel {
                request_id: 1,
                user_identity: "farm-07".to_string(),
                nominal_hash_rate: 40.0,
                max_target: [0xFF; 32],
            }),
        ),
        (
            // A nonce range at the top of the space, so the mutator starts one
            // bit away from the u64 boundary the range check has to hold at.
            "open_channel_success",
            Message::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccess {
                request_id: 1,
                channel_id: 1,
                target: [0x0F; 32],
                nonce_min: u64::MAX - 1,
                nonce_max: u64::MAX,
            }),
        ),
        (
            "new_mining_job",
            Message::NewMiningJob(NewMiningJob {
                channel_id: 1,
                job_id: 1,
                state_root: [0xAB; 32],
                timestamp: u64::MAX,
            }),
        ),
        (
            // An all-zero target: the hardest possible, and the value a
            // truncation bug is most likely to produce by accident.
            "set_new_prev_hash",
            Message::SetNewPrevHash(SetNewPrevHash {
                channel_id: 1,
                job_id: 1,
                prev_hash: [0xCD; 32],
                min_ntime: 1_800_000_000,
                target: [0x00; 32],
            }),
        ),
        (
            "set_target",
            Message::SetTarget(SetTarget {
                channel_id: 1,
                maximum_target: [0xFF; 32],
            }),
        ),
        (
            // The message the pool sees more than every other combined.
            "submit_shares",
            Message::SubmitSharesStandard(SubmitSharesStandard {
                channel_id: 1,
                sequence_number: u32::MAX,
                job_id: 1,
                nonce: u64::MAX,
                ntime: 1_800_000_000,
            }),
        ),
        (
            // A string field, so the mutator has a length prefix to lie about.
            "close_channel",
            Message::CloseChannel(CloseChannel {
                channel_id: 1,
                reason_code: "shutting-down".to_string(),
            }),
        ),
        (
            // The only message with range-checked scalar fields: fan speed is
            // refused above 100 and the sample window above zero. A mutator
            // flipping bits in either must reach an error rather than a value
            // the dashboard divides by.
            "worker_telemetry",
            Message::SubmitWorkerTelemetry(SubmitWorkerTelemetry {
                channel_id: 1,
                power_milliwatts: u64::MAX,
                // Signed, and at the edge: the field exists because immersion
                // rigs run below zero, and a mutator should be able to walk
                // across the sign boundary from a seed that is already there.
                temperature_millicelsius: i32::MIN,
                fan_percent: 100,
                sample_millis: 1,
            }),
        ),
    ];

    messages
        .into_iter()
        .map(|(name, message)| {
            let frame = message.to_frame().expect("seed messages must encode");
            (name.to_string(), frame.encode())
        })
        .collect()
}
