//! The node's half of the Ledger parity contract.
//!
//! `apps/ledger-maya2c` is not a workspace member — it targets a Cortex-M and
//! must not link `RocksDB` — so it cannot call the node to check itself.
//! Instead this test produces, with the node's own code, a suite-`0x10`
//! transfer: chain key, public key, address, the exact bytes a v7 signature
//! covers, and the deterministic signature. It writes them to
//! `apps/ledger-maya2c/tests/fixtures/suite-0x10-transfer.txt`, and the
//! device's `tests/parity_tests.rs` must reproduce every byte.
//!
//! The file is checked in. Here it is compared, not rewritten, unless
//! `MAYA_WRITE_LEDGER_FIXTURE=1` is set: a node change that alters any of
//! these bytes fails here first, and the author then regenerates the fixture
//! and watches the device side follow, or not.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use custom_l1_node::core::transaction::{Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::suites;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/ledger-maya2c/tests/fixtures/suite-0x10-transfer.txt")
}

/// The fixture, as the node computes it today.
fn fixture() -> String {
    let chain_key = [0x11u8; 32];
    let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(chain_key));
    let public_key = MlDsa65::public_key(&key);

    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [0x22; 32],
            index: 1,
        }],
        vec![
            TxOutput {
                amount: 1_250_000,
                recipient: [0x33; 32],
            },
            TxOutput {
                amount: 40_000,
                recipient: [0x44; 32],
            },
        ],
        7,
    );
    tx.sign_with_suite::<MlDsa65>(&key).expect("sign");
    let auth = tx.suite_auth.as_ref().expect("v7");
    let signature = auth.signature.as_ref().expect("signed");
    assert_eq!(
        tx.sender(),
        suites::suite_address(SuiteId::MlDsa65, &public_key)
    );

    let mut out = String::new();
    for (key, value) in [
        ("chain_key", hex::encode(chain_key)),
        ("public_key", hex::encode(&public_key)),
        ("address", hex::encode(tx.sender())),
        ("signing_bytes", hex::encode(tx.signing_bytes())),
        ("signature", hex::encode(signature)),
    ] {
        let _ = writeln!(out, "{key}={value}");
    }
    out
}

#[test]
fn the_apps_coin_type_matches_the_wallets() {
    // `apps/ledger-maya2c/src/derive.rs` duplicates it (the app cannot depend
    // on the wallet); a device deriving on another coin type produces keys the
    // desktop wallet cannot find.
    const APP_COIN_TYPE: u32 = 7331;
    assert_eq!(APP_COIN_TYPE, maya_wallet_core::hd::COIN_TYPE);
}

#[test]
fn the_ledger_parity_fixture_is_what_the_node_computes() {
    let expected = fixture();
    let path = fixture_path();
    if std::env::var("MAYA_WRITE_LEDGER_FIXTURE").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("dir")).expect("mkdir");
        std::fs::write(&path, &expected).expect("write fixture");
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (set MAYA_WRITE_LEDGER_FIXTURE=1)", path.display()));
    assert!(
        on_disk.replace("\r\n", "\n") == expected,
        "the node no longer produces the Ledger fixture; regenerate it with \
         MAYA_WRITE_LEDGER_FIXTURE=1 and rerun apps/ledger-maya2c's parity tests"
    );
}
