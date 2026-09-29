//! ADR-007 on the node: the `0x30` suite is today's hybrid byte for byte, and
//! suite-tagged (v7) transactions verify from genesis through `verify_at`
//! alone (ADR-013).
//!
//! The parity half is what lets the envelope land without migrating a single
//! account: the node signs its hybrid half with `fips204`, the suite with
//! RustCrypto `ml-dsa`, and both must be the same FIPS 204 function of the
//! same seed. If they ever diverge, this fails before any chain does.

use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::hybrid;
use custom_l1_node::crypto::suites;
use custom_l1_node::governance::ParameterTable;
use custom_l1_node::state::context::SUITE_ENVELOPE_ACTIVATION_HEIGHT;
use maya_crypto_pq::agility::{MIN_EMERGENCY_WINDOW, Network, SuitePolicy};
use maya_crypto_pq::envelope::SignedEnvelope;
use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa87, SignatureSuite, SuiteId,
};
use maya_governance::params::ParameterKey;

fn chain_key(byte: u8) -> [u8; 32] {
    let mut key = [byte; 32];
    key[31] = byte.wrapping_mul(7);
    key
}

#[test]
fn suite_0x30_derives_the_node_hybrid_key_byte_for_byte() {
    for byte in [0u8, 1, 42, 255] {
        let node = hybrid::signing_key_from_seed(&chain_key(byte)).expect("node key");
        let mut node_pk = Vec::new();
        node.public_key().encode_into(&mut node_pk);

        let suite_key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(
            chain_key(byte),
        ));
        assert_eq!(
            HybridMlDsa65SlhDsa128s::public_key(&suite_key),
            node_pk,
            "seed byte {byte}"
        );
    }
}

#[test]
fn suite_0x30_and_the_node_produce_identical_signatures() {
    let seed = chain_key(9);
    let node = hybrid::signing_key_from_seed(&seed).expect("node key");
    let suite_key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(seed));
    for message in [b"".as_slice(), b"maya2c", &[0xA5; 4096]] {
        let node_sig = node.sign(message).expect("node sign");
        let mut node_bytes = Vec::new();
        node_sig.encode_into(&mut node_bytes);
        let suite_sig = HybridMlDsa65SlhDsa128s::sign(&suite_key, message).expect("suite sign");
        assert_eq!(
            suite_sig, node_bytes,
            "deterministic FIPS 204/205 signing must agree"
        );
    }
}

#[test]
fn every_signed_hybrid_transaction_lifts_into_a_valid_0x30_envelope() {
    let key = hybrid::signing_key_from_seed(&chain_key(3)).expect("key");
    let mut tx = Transaction::new(Vec::new(), Vec::new(), 7);
    tx.sign(&key).expect("sign");

    let mut pk = Vec::new();
    tx.public_key.encode_into(&mut pk);
    let mut sig = Vec::new();
    tx.signature
        .as_deref()
        .expect("signed")
        .encode_into(&mut sig);
    let envelope = SignedEnvelope::from_legacy_hybrid(&pk, &sig).expect("lift");
    assert_eq!(envelope.verify(&tx.signing_bytes()), Ok(()));
}

fn v7_transfer() -> (Transaction, <MlDsa87 as SignatureSuite>::SigningKey) {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([4; 32]));
    let mut tx = Transaction::new(Vec::new(), Vec::new(), 1);
    tx.sign_with_suite::<MlDsa87>(&key).expect("sign");
    (tx, key)
}

#[test]
fn a_v7_transaction_round_trips_and_names_its_suite() {
    let (tx, _) = v7_transfer();
    let bytes = tx.to_bytes();
    assert_eq!(bytes[0], 7);
    let back = Transaction::from_bytes(&bytes).expect("decode");
    assert_eq!(back, tx);
    assert_eq!(back.txid(), tx.txid());
    assert_eq!(
        back.suite_auth.as_deref().map(|a| a.suite),
        Some(SuiteId::MlDsa87)
    );
    let pk = &tx.suite_auth.as_deref().expect("auth").public_key;
    assert_eq!(tx.sender(), suites::suite_address(SuiteId::MlDsa87, pk));
}

#[test]
fn a_v7_frame_with_a_mismatched_length_or_trailing_bytes_is_refused() {
    let (tx, _) = v7_transfer();
    let good = tx.to_bytes();
    let mut trailing = good.clone();
    trailing.push(0);
    assert!(Transaction::from_bytes(&trailing).is_err());
    for cut in [1, good.len() / 2, good.len() - 1] {
        assert!(Transaction::from_bytes(&good[..cut]).is_err(), "cut {cut}");
    }
}

#[test]
fn v7_is_live_from_genesis_and_still_refused_by_verify() {
    assert_eq!(
        SUITE_ENVELOPE_ACTIVATION_HEIGHT, 0,
        "ADR-013: live from genesis"
    );
    let (tx, _) = v7_transfer();
    let policy = suites::verification_policy();
    assert!(
        tx.verify().is_err(),
        "verify() has no height, so it never accepts a suite-tagged transaction"
    );
    for height in [0, 1_000_000, u64::MAX - 1] {
        assert_eq!(tx.verify_at(height, &policy), Ok(()), "height {height}");
    }
}

#[test]
fn consensus_verifies_under_mainnet_rules_whatever_the_network() {
    // Devnet's policy admits 0x01, which has no post-quantum security; the
    // policy consensus uses must not, or a node's config would pick its rules.
    let consensus = suites::verification_policy();
    assert!(!consensus.permitted(SuiteId::Ed25519));
    assert!(SuitePolicy::genesis(Network::Devnet).permitted(SuiteId::Ed25519));
    for suite in [
        SuiteId::MlDsa65,
        SuiteId::MlDsa87,
        SuiteId::SlhDsaSha2_128s,
        SuiteId::SlhDsaShake256f,
        SuiteId::HybridMlDsa65SlhDsa128s,
    ] {
        assert!(consensus.may_sign(suite, 0), "{suite:?}");
    }
}

#[test]
fn past_activation_v7_verifies_and_the_policy_applies() {
    let at = SUITE_ENVELOPE_ACTIVATION_HEIGHT;
    let (tx, _) = v7_transfer();
    let policy = SuitePolicy::genesis(Network::Mainnet);
    assert_eq!(tx.verify_at(at, &policy), Ok(()));

    let mut tampered = tx.clone();
    tampered.nonce += 1;
    assert!(tampered.verify_at(at, &policy).is_err());

    let deprecated = policy
        .with_default(SuiteId::MlDsa65)
        .and_then(|p| p.with_deprecation(SuiteId::MlDsa87, 0, MIN_EMERGENCY_WINDOW, true))
        .expect("policy");
    // Deprecated at 0 with the emergency window: it still signs inside the
    // window (that is what a migration window is for) and not after it.
    assert_eq!(
        tx.verify_at(at, &deprecated),
        Ok(()),
        "inside the migration window the suite still signs"
    );
    assert!(
        tx.verify_at(at + MIN_EMERGENCY_WINDOW, &deprecated)
            .is_err(),
        "a sunset suite cannot sign"
    );

    // A hybrid transaction still takes the hybrid rule through verify_at.
    let key = hybrid::signing_key_from_seed(&chain_key(5)).expect("key");
    let mut legacy = Transaction::new(Vec::new(), Vec::new(), 2);
    legacy.sign(&key).expect("sign");
    assert_eq!(legacy.verify_at(0, &policy), Ok(()));
}

#[test]
fn governance_default_suite_is_ml_dsa_87_and_is_checked_by_the_node() {
    let table = ParameterTable::new();
    assert_eq!(
        suites::default_suite(&table).expect("default"),
        SuiteId::MlDsa87
    );
    let hybrid = table.with(ParameterKey::DefaultSignatureSuite, 0x30);
    assert_eq!(
        suites::default_suite(&hybrid).expect("0x30"),
        SuiteId::HybridMlDsa65SlhDsa128s
    );
    // Inside the governance range, but no suite: refused by the node.
    let hole = table.with(ParameterKey::DefaultSignatureSuite, 0x12);
    assert!(suites::default_suite(&hole).is_err());
    assert!(suites::policy(&table).is_ok());
    // The same hole is refused at proposal and execution time.
    assert!(suites::check_parameter(ParameterKey::DefaultSignatureSuite, 0x12).is_err());
    assert!(suites::check_parameter(ParameterKey::DefaultSignatureSuite, 0x21).is_ok());
    assert!(suites::check_parameter(ParameterKey::DexProtocolFeeBps, 0x12).is_ok());
    // The governance range itself excludes Ed25519.
    assert!(ParameterKey::DefaultSignatureSuite.check(0x01).is_err());
}

#[test]
fn every_registry_length_fits_the_u32_length_prefix() {
    // `suite_tx::write_len` relies on this; the assertion makes its `expect`
    // provably unreachable rather than assumed.
    for id in SuiteId::ALL {
        let info = id.info();
        assert!(
            u32::try_from(info.public_key_len).is_ok() && u32::try_from(info.signature_len).is_ok()
        );
    }
}

#[test]

mod common;
fn a_failed_suite_signature_leaves_the_transaction_untouched_and_rpc_reports_the_suite() {
    use custom_l1_node::rpc::TransactionInfo;
    let (tx, _) = v7_transfer();
    let info = TransactionInfo::from(&tx);
    let suite = info.suite.expect("v7 reports its suite");
    assert_eq!(suite.id, SuiteId::MlDsa87.to_byte());
    assert!(info.signed && info.lattice_public_key.is_empty());

    let key = hybrid::signing_key_from_seed(&chain_key(6)).expect("key");
    let mut legacy = Transaction::new(Vec::new(), Vec::new(), 3);
    legacy.sign(&key).expect("sign");
    let legacy_info = TransactionInfo::from(&legacy);
    assert!(legacy_info.suite.is_none() && !legacy_info.lattice_public_key.is_empty());
}
