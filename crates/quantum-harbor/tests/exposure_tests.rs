#![allow(clippy::unwrap_used, clippy::similar_names)]

use maya_quantum_harbor::{
    Exposure, Template, bitcoin_exposure, ethereum_exposure, explain, parse, template,
};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Bitcoin's genesis coinbase transaction, raw. Its id must equal the merkle
/// root of the real genesis header, which `crates/btc-spv/tests/spv_tests.rs`
/// verifies: `4a5e1e4b…a33b`. That equality is what makes this real data.
const GENESIS_COINBASE: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";
const GENESIS_MERKLE_ROOT: &str =
    "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b";

#[test]
fn bitcoins_genesis_output_is_a_p2pk_with_its_public_key_exposed() {
    let tx = parse(&unhex(GENESIS_COINBASE)).unwrap();
    let id = tx.txid_display.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
        s
    });
    assert_eq!(
        id, GENESIS_MERKLE_ROOT,
        "the parsed transaction is the real genesis coinbase"
    );
    assert_eq!(tx.outputs.len(), 1);
    let (value, script) = &tx.outputs[0];
    assert_eq!(*value, 5_000_000_000, "50 BTC");
    assert_eq!(template(script), Template::P2pk);
    assert_eq!(bitcoin_exposure(script), Exposure::Exposed);
    println!(
        "genesis coinbase {id}: 50 BTC in P2PK — {}",
        explain(Exposure::Exposed)
    );
}

#[test]
fn each_template_is_classified_by_exact_shape() {
    let h20 = [0x11u8; 20];
    let h32 = [0x22u8; 32];
    let p2pkh = [&[0x76, 0xa9, 0x14][..], &h20, &[0x88, 0xac]].concat();
    let p2sh = [&[0xa9, 0x14][..], &h20, &[0x87]].concat();
    let p2wpkh = [&[0x00, 0x14][..], &h20].concat();
    let p2wsh = [&[0x00, 0x20][..], &h32].concat();
    let p2tr = [&[0x51, 0x20][..], &h32].concat();
    let compressed = [&[0x21][..], &[0x02; 33], &[0xac]].concat();
    assert_eq!(bitcoin_exposure(&p2pkh), Exposure::HashedUntilSpent);
    assert_eq!(bitcoin_exposure(&p2wpkh), Exposure::HashedUntilSpent);
    assert_eq!(bitcoin_exposure(&p2sh), Exposure::DependsOnScript);
    assert_eq!(bitcoin_exposure(&p2wsh), Exposure::DependsOnScript);
    assert_eq!(
        bitcoin_exposure(&p2tr),
        Exposure::Exposed,
        "a Taproot output key is a public key"
    );
    assert_eq!(bitcoin_exposure(&compressed), Exposure::Exposed);
    assert_eq!(bitcoin_exposure(&[0x6a, 0x04, 1, 2, 3, 4]), Exposure::NoKey);
    assert_eq!(
        bitcoin_exposure(&[0x52, 0x53]),
        Exposure::Unknown,
        "no guess for a non-template"
    );
    assert_eq!(
        bitcoin_exposure(&p2pkh[..p2pkh.len() - 1]),
        Exposure::Unknown,
        "a truncated template is not that template"
    );
}

#[test]
fn an_ethereum_account_is_exposed_once_it_has_signed() {
    assert_eq!(ethereum_exposure(false, 0), Exposure::HashedUntilSpent);
    assert_eq!(ethereum_exposure(false, 1), Exposure::Exposed);
    assert_eq!(ethereum_exposure(true, 5), Exposure::NoKey);
}

#[test]
fn malformed_transactions_are_refused_not_guessed() {
    let raw = unhex(GENESIS_COINBASE);
    assert!(parse(&raw[..raw.len() - 1]).is_none());
    assert!(parse(&[raw.as_slice(), &[0]].concat()).is_none());
}
