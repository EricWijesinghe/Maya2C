//! The IoT anchor end to end: enrollment, telemetry, PUF-derived identity,
//! tamper and clone evidence, revocation and reorgs, through the real apply
//! path of a `StateDB`.
//!
//! ## What "physical tamper detection on-chain" can mean
//!
//! A chain senses nothing. What these tests pin is every signal it *can*
//! record: a tamper event the device signed before zeroizing its seed, a fork
//! of the device's batch hash chain that only a copied key produces, and — as
//! the one thing it cannot tell from honesty — a false reading signed by a
//! genuine device, which is recorded exactly as signed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use tempfile::TempDir;

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::Mempool;
use custom_l1_node::state::{Account, BlockContext, StateDB};
use maya_iot_anchor::merkle::ReadingsAccumulator;
use maya_iot_anchor::puf::{CORRECTABLE_PER_GROUP, HelperData, REPETITION, RESPONSE_BYTES};
use maya_iot_anchor::{
    Bounds, DeviceKey, DeviceRecord, DeviceStatus, Equivocation, GENESIS_PREVIOUS, IotError, Seed,
    SensorClass, TamperCause, TelemetryBatch,
};

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// The IoT anchor switched on. Everywhere the node builds a context it is off.
fn active(height: u64) -> BlockContext {
    BlockContext::at_height(height).with_iot_activation(0)
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_789_400_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

struct Chain {
    db: Arc<StateDB>,
    height: u64,
    _dir: TempDir,
}

impl Chain {
    fn new(funded: &[&HybridSigningKey]) -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = StateDB::open(dir.path()).expect("open");
        for key in funded {
            let account = Account {
                balance: 1_000_000,
                nonce: 0,
            };
            db.put_account(&key.address(), &account).expect("fund");
        }
        Self {
            db: Arc::new(db),
            height: 0,
            _dir: dir,
        }
    }

    fn signed(&self, kind: TxKind, key: &HybridSigningKey) -> Transaction {
        let nonce = self.db.get_account(&key.address()).expect("account").nonce;
        let mut tx = Transaction::with_kind(kind, nonce);
        tx.sign(key).expect("sign");
        tx
    }

    /// Applies one transaction as the next block, under `context_of(height)`.
    fn apply_with(
        &mut self,
        kind: TxKind,
        key: &HybridSigningKey,
        context_of: fn(u64) -> BlockContext,
    ) -> Result<[u8; 32], NodeError> {
        let tx = self.signed(kind, key);
        let result = self
            .db
            .apply_block(&block_of(vec![tx]), context_of(self.height + 1));
        if result.is_ok() {
            self.height += 1;
        }
        result
    }

    fn apply(&mut self, kind: TxKind, key: &HybridSigningKey) -> Result<[u8; 32], NodeError> {
        self.apply_with(kind, key, active)
    }

    /// Relays a batch through `gateway`. Relays never fail for losing.
    fn relay(&mut self, batch: &TelemetryBatch, gateway: &HybridSigningKey) {
        self.apply(TxKind::SubmitTelemetry(Box::new(batch.clone())), gateway)
            .expect("a relayed batch that verifies is never an error");
    }

    fn device(&self, key: &DeviceKey) -> DeviceRecord {
        self.db
            .stored_iot_device(&key.id())
            .expect("read")
            .expect("enrolled")
    }

    fn root(&self) -> [u8; 32] {
        self.db.state_root().expect("root")
    }
}

fn rng(seed: u64) -> ChaCha20Rng {
    ChaCha20Rng::seed_from_u64(seed)
}

fn device(byte: u8) -> DeviceKey {
    DeviceKey::from_seed(&Seed::new([byte; 32]))
}

const METER: Bounds = Bounds {
    min: 0,
    max: 5_000_000,
    max_step: 200_000,
};

const COLD_CHAIN: Bounds = Bounds {
    min: -40_000,
    max: 8_000,
    max_step: 5_000,
};

/// A batch of `values` from counter `first`, signed as the successor of
/// `previous` (the hash of the device's last batch, or [`GENESIS_PREVIOUS`]).
fn batch(
    key: &DeviceKey,
    previous: [u8; 32],
    first: u64,
    values: &[i64],
    rng: &mut ChaCha20Rng,
) -> TelemetryBatch {
    let mut readings = ReadingsAccumulator::new();
    for (offset, value) in (0u64..).zip(values) {
        readings.push(first + offset, *value).expect("contiguous");
    }
    let summary = readings.summary().expect("non-empty");
    key.sign_batch(&summary, &previous, rng).expect("sign")
}

fn enroll(
    chain: &mut Chain,
    owner: &HybridSigningKey,
    key: &DeviceKey,
    class: SensorClass,
    bounds: Bounds,
    rng: &mut ChaCha20Rng,
) {
    let enrollment = key
        .enroll(&owner.address(), class, bounds, rng)
        .expect("sign");
    chain
        .apply(TxKind::EnrollDevice(Box::new(enrollment)), owner)
        .expect("enroll");
}

fn is_iot_refusal(result: &Result<[u8; 32], NodeError>) -> bool {
    matches!(result, Err(NodeError::Iot(_)))
}

// ---------------------------------------------------------------------------
// enrollment
// ---------------------------------------------------------------------------

#[test]
fn enrollment_binds_a_device_to_its_owner_and_refuses_what_it_cannot_prove() {
    // Arrange
    let owner = generate_signing_key().expect("keygen");
    let rival = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &rival]);
    let mut rng = rng(1);
    let meter = device(1);
    let for_owner = meter
        .enroll(&owner.address(), SensorClass::EnergyMeter, METER, &mut rng)
        .expect("sign");
    let mempool = Mempool::new(Arc::clone(&chain.db));
    let root = chain.root();

    // Act / Assert: dark before activation.
    let dark = chain.apply_with(
        TxKind::EnrollDevice(Box::new(for_owner.clone())),
        &owner,
        BlockContext::at_height,
    );
    assert!(is_iot_refusal(&dark), "{dark:?}");

    // A proof made out to the owner does not enroll the device for a rival.
    let stolen = chain.signed(TxKind::EnrollDevice(Box::new(for_owner.clone())), &rival);
    assert!(matches!(mempool.validate(&stolen), Err(NodeError::Iot(_))));
    let hijack = chain.apply(TxKind::EnrollDevice(Box::new(for_owner.clone())), &rival);
    assert!(is_iot_refusal(&hijack), "{hijack:?}");
    assert_eq!(chain.root(), root, "a refused enrollment moved the root");

    // The owner enrolls it.
    chain
        .apply(TxKind::EnrollDevice(Box::new(for_owner)), &owner)
        .expect("enroll");
    let record = chain.device(&meter);
    assert_eq!(record.owner, owner.address());
    assert_eq!(record.status, DeviceStatus::Active);
    assert_eq!(record.class, SensorClass::EnergyMeter);
    assert!(!record.progress.has_batch);

    // A second enrollment of the same key, even with a valid proof, is refused.
    let again = meter
        .enroll(&rival.address(), SensorClass::EnergyMeter, METER, &mut rng)
        .expect("sign");
    let duplicate = chain.apply(TxKind::EnrollDevice(Box::new(again)), &rival);
    assert!(is_iot_refusal(&duplicate), "{duplicate:?}");
    assert_eq!(chain.device(&meter).owner, owner.address());
}

// ---------------------------------------------------------------------------
// telemetry
// ---------------------------------------------------------------------------

#[test]
fn a_hash_chain_of_batches_is_recorded_and_replays_stale_and_early_relays_are_no_ops() {
    // Arrange: an energy meter and a cold-chain logger, one gateway relaying both.
    let owner = generate_signing_key().expect("keygen");
    let gateway = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &gateway]);
    let mut rng = rng(2);
    let (meter, logger) = (device(2), device(3));
    enroll(
        &mut chain,
        &owner,
        &meter,
        SensorClass::EnergyMeter,
        METER,
        &mut rng,
    );
    enroll(
        &mut chain,
        &owner,
        &logger,
        SensorClass::ColdChainTemperature,
        COLD_CHAIN,
        &mut rng,
    );

    let first = batch(
        &meter,
        GENESIS_PREVIOUS,
        1,
        &[1_000, 1_250, 1_500],
        &mut rng,
    );
    let cold = batch(
        &logger,
        GENESIS_PREVIOUS,
        1,
        &[-18_000, -17_950, -18_020],
        &mut rng,
    );

    // Act: record both.
    chain.relay(&first, &gateway);
    chain.relay(&cold, &gateway);

    // Assert
    let recorded = chain.device(&meter).progress;
    assert_eq!(
        (
            recorded.first,
            recorded.last,
            recorded.batches,
            recorded.anomalies
        ),
        (1, 3, 1, 0)
    );
    assert_eq!((recorded.min, recorded.max), (1_000, 1_500));
    assert_eq!(chain.device(&logger).progress.batches, 1);

    // A second gateway relays the same batch — a no-op, not an error.
    chain.relay(&first, &gateway);
    assert_eq!(chain.device(&meter).progress, recorded);

    // The next batch in the chain, far outside the declared range: recorded,
    // flagged.
    let spike = batch(&meter, first.hash(), 4, &[9_000_000], &mut rng);
    // Relayed before its predecessor would be recorded? Here, a batch naming an
    // unrecorded predecessor is simply not linked yet — a no-op.
    let early = batch(&meter, spike.hash(), 5, &[1_600], &mut rng);
    chain.relay(&early, &gateway);
    assert_eq!(chain.device(&meter).progress, recorded);
    chain.relay(&spike, &gateway);
    let flagged = chain.device(&meter).progress;
    assert_eq!(
        (flagged.last, flagged.batches, flagged.anomalies),
        (4, 2, 1)
    );
    // The gateway retries the early batch; now it links.
    chain.relay(&early, &gateway);
    assert_eq!(chain.device(&meter).progress.last, 5);

    // A late relay of an old batch is stale: a no-op.
    let head = chain.device(&meter).progress;
    chain.relay(&first, &gateway);
    assert_eq!(chain.device(&meter).progress, head);

    // A byte-flipped batch is the relay's error.
    let mut forged = batch(&meter, early.hash(), 6, &[1_700], &mut rng);
    forged.max += 1;
    let refused = chain.apply(TxKind::SubmitTelemetry(Box::new(forged)), &gateway);
    assert!(is_iot_refusal(&refused), "{refused:?}");

    // A batch for a device nobody enrolled is a no-op.
    chain.relay(
        &batch(&device(99), GENESIS_PREVIOUS, 1, &[1], &mut rng),
        &gateway,
    );

    // Signed is not true: a genuine device signing a plausible false reading is
    // recorded exactly as signed — the chain cannot tell.
    let false_reading = batch(&logger, cold.hash(), 4, &[-18_010], &mut rng);
    chain.relay(&false_reading, &gateway);
    let logged = chain.device(&logger).progress;
    assert_eq!((logged.batches, logged.anomalies), (2, 0));
}

// ---------------------------------------------------------------------------
// PUF identity
// ---------------------------------------------------------------------------

#[test]
fn a_puf_device_keeps_its_identity_through_noise_and_loses_it_past_the_budget() {
    // Arrange: manufacture binds a TRNG secret to the chip's response.
    let owner = generate_signing_key().expect("keygen");
    let gateway = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &gateway]);
    let mut rng = rng(3);
    let mut response = [0u8; RESPONSE_BYTES];
    rng.fill_bytes(&mut response);
    let mut secret = [0u8; 32];
    rng.fill_bytes(&mut secret);
    let helper = HelperData::enroll(&response, &secret);
    let enrolled = DeviceKey::from_seed(&helper.reproduce(&response).expect("clean"));
    enroll(
        &mut chain,
        &owner,
        &enrolled,
        SensorClass::ColdChainTemperature,
        COLD_CHAIN,
        &mut rng,
    );

    let noisy = |flips: usize| {
        let mut out = response;
        for group in 0..256 {
            for copy in 0..flips {
                let index = group * REPETITION + copy;
                out[index / 8] ^= 1 << (index % 8);
            }
        }
        out
    };

    // Act: a warm boot with the worst correctable noise.
    let rebooted = DeviceKey::from_seed(
        &helper
            .reproduce(&noisy(CORRECTABLE_PER_GROUP))
            .expect("within budget"),
    );
    let reading = batch(&rebooted, GENESIS_PREVIOUS, 1, &[-18_000], &mut rng);

    // Assert: same device, and the chain accepts its signature.
    assert_eq!(rebooted.id(), enrolled.id());
    chain.relay(&reading, &gateway);
    assert_eq!(chain.device(&enrolled).progress.batches, 1);

    // Past the budget the device cannot derive a key at all — an error, never a
    // different key that could sign for someone else.
    assert_eq!(
        helper.reproduce(&noisy(CORRECTABLE_PER_GROUP + 1)).err(),
        Some(IotError::PufUncorrectable)
    );

    // A different chip, pretending: its key signs under the enrolled id and fails.
    let mut claimed = batch(&device(77), reading.hash(), 2, &[-18_000], &mut rng);
    claimed.device = enrolled.id();
    let refused = chain.apply(TxKind::SubmitTelemetry(Box::new(claimed)), &gateway);
    assert!(is_iot_refusal(&refused), "{refused:?}");
}

// ---------------------------------------------------------------------------
// tamper and clones
// ---------------------------------------------------------------------------

#[test]
fn a_signed_tamper_event_retires_the_device_for_good() {
    // Arrange
    let owner = generate_signing_key().expect("keygen");
    let gateway = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &gateway]);
    let mut rng = rng(4);
    let logger = device(4);
    enroll(
        &mut chain,
        &owner,
        &logger,
        SensorClass::ColdChainTemperature,
        COLD_CHAIN,
        &mut rng,
    );
    let first = batch(&logger, GENESIS_PREVIOUS, 1, &[-18_000], &mut rng);
    chain.relay(&first, &gateway);

    // Act: the enclosure opens; firmware signs, then zeroizes its seed.
    let event = logger
        .sign_tamper(2, TamperCause::Enclosure, &mut rng)
        .expect("sign");
    chain
        .apply(TxKind::ReportTamper(Box::new(event)), &gateway)
        .expect("tamper");

    // Assert
    let retired = chain.device(&logger);
    assert_eq!(retired.status, DeviceStatus::Tampered);

    // Nothing moves it again: telemetry, a second tamper, even the owner's revoke.
    chain.relay(
        &batch(&logger, first.hash(), 3, &[-18_000], &mut rng),
        &gateway,
    );
    let again = logger
        .sign_tamper(4, TamperCause::Light, &mut rng)
        .expect("sign");
    chain
        .apply(TxKind::ReportTamper(Box::new(again)), &gateway)
        .expect("no-op");
    chain
        .apply(TxKind::RevokeDevice(logger.id()), &owner)
        .expect("no-op");
    assert_eq!(chain.device(&logger), retired);

    // A forged tamper report — someone else's key — is refused.
    let mut claimed = device(40)
        .sign_tamper(5, TamperCause::Enclosure, &mut rng)
        .expect("sign");
    claimed.device = logger.id();
    let refused = chain.apply(TxKind::ReportTamper(Box::new(claimed)), &gateway);
    assert!(is_iot_refusal(&refused), "{refused:?}");
}

#[test]
fn a_clone_that_jumps_ahead_is_caught_by_the_fork_it_makes() {
    // Arrange: a meter whose seed was extracted. The clone posts a batch far
    // ahead of the genuine counter so no range ever overlaps — the attack that
    // would mute the genuine device if batches were judged by counters alone.
    let owner = generate_signing_key().expect("keygen");
    let gateway = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &gateway]);
    let mut rng = rng(5);
    let (genuine, clone) = (device(5), device(5));
    enroll(
        &mut chain,
        &owner,
        &genuine,
        SensorClass::EnergyMeter,
        METER,
        &mut rng,
    );
    let g1 = batch(&genuine, GENESIS_PREVIOUS, 1, &[10, 20, 30], &mut rng);
    chain.relay(&g1, &gateway);

    // Act: the clone extends the recorded head, a thousand counters ahead.
    let c1 = batch(&clone, g1.hash(), 1_000, &[999, 1_000], &mut rng);
    chain.relay(&c1, &gateway);
    assert_eq!(chain.device(&genuine).status, DeviceStatus::Active);

    // The genuine device, unaware, extends its own last batch — the same
    // predecessor the clone's recorded batch named.
    let g2 = batch(&genuine, g1.hash(), 4, &[40], &mut rng);
    chain.relay(&g2, &gateway);

    // Assert: two successors of one predecessor. Only a copied key signs both.
    assert_eq!(chain.device(&genuine).status, DeviceStatus::Compromised);
}

#[test]
fn a_clone_that_races_further_ahead_is_caught_by_filed_evidence() {
    // Arrange: the clone extends the head twice before the genuine device's next
    // batch arrives, so the chain's head no longer names the forked predecessor.
    let owner = generate_signing_key().expect("keygen");
    let gateway = generate_signing_key().expect("keygen");
    let watcher = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &gateway, &watcher]);
    let mut rng = rng(6);
    let (genuine, clone) = (device(6), device(6));
    enroll(
        &mut chain,
        &owner,
        &genuine,
        SensorClass::EnergyMeter,
        METER,
        &mut rng,
    );
    let g1 = batch(&genuine, GENESIS_PREVIOUS, 1, &[5, 6], &mut rng);
    chain.relay(&g1, &gateway);
    let c1 = batch(&clone, g1.hash(), 100, &[7], &mut rng);
    let c2 = batch(&clone, c1.hash(), 101, &[8], &mut rng);
    chain.relay(&c1, &gateway);
    chain.relay(&c2, &gateway);

    // Act 1: the genuine batch is judged stale by state alone.
    let g2 = batch(&genuine, g1.hash(), 3, &[9], &mut rng);
    chain.relay(&g2, &gateway);
    assert_eq!(chain.device(&genuine).status, DeviceStatus::Active);

    // Act 2: the owner's gateway, seeing its batch not recorded, files the two
    // batches that share a predecessor. A non-conflicting pair is refused first.
    let not_conflicting = Equivocation {
        first: g1.clone(),
        second: c1.clone(),
    };
    let refused = chain.apply(
        TxKind::ProveEquivocation(Box::new(not_conflicting)),
        &watcher,
    );
    assert!(is_iot_refusal(&refused), "{refused:?}");
    let evidence = Equivocation {
        first: c1,
        second: g2.clone(),
    };
    chain
        .apply(TxKind::ProveEquivocation(Box::new(evidence)), &watcher)
        .expect("evidence");

    // Assert
    assert_eq!(chain.device(&genuine).status, DeviceStatus::Compromised);
    let frozen = chain.device(&genuine);
    chain.relay(&batch(&clone, c2.hash(), 102, &[10], &mut rng), &gateway);
    assert_eq!(
        chain.device(&genuine),
        frozen,
        "a compromised device records nothing"
    );
}

// ---------------------------------------------------------------------------
// revocation and reorgs
// ---------------------------------------------------------------------------

#[test]
fn only_the_owner_revokes_and_a_reverted_block_restores_the_device() {
    // Arrange
    let owner = generate_signing_key().expect("keygen");
    let gateway = generate_signing_key().expect("keygen");
    let stranger = generate_signing_key().expect("keygen");
    let mut chain = Chain::new(&[&owner, &gateway, &stranger]);
    let mut rng = rng(7);
    let meter = device(8);
    enroll(
        &mut chain,
        &owner,
        &meter,
        SensorClass::EnergyMeter,
        METER,
        &mut rng,
    );
    let before = chain.device(&meter);
    let root = chain.root();

    // Act: a batch in a journaled block, then a reorg that takes it back.
    let first = batch(&meter, GENESIS_PREVIOUS, 1, &[100, 200], &mut rng);
    let tx = chain.signed(TxKind::SubmitTelemetry(Box::new(first)), &gateway);
    let mut block = block_of(vec![tx]);
    let context = active(chain.height + 1);
    block.header.state_root = chain.db.preview_root(&block, context).expect("preview");
    chain
        .db
        .apply_block_journaled(&block, &[0xB1; 32], context)
        .expect("apply");
    assert!(chain.device(&meter).progress.has_batch);
    chain.db.revert_block(&[0xB1; 32]).expect("revert");

    // Assert
    assert_eq!(chain.device(&meter), before);
    assert_eq!(chain.root(), root);

    // A stranger cannot revoke; the owner can.
    let refused = chain.apply(TxKind::RevokeDevice(meter.id()), &stranger);
    assert!(is_iot_refusal(&refused), "{refused:?}");
    chain
        .apply(TxKind::RevokeDevice(meter.id()), &owner)
        .expect("revoke");
    assert_eq!(chain.device(&meter).status, DeviceStatus::Revoked);
}
