//! The device app under the Speculos emulator: every command, end to end.
//!
//! Run by `scripts/ledger_speculos.sh`, which builds the app for Nano S Plus
//! (`cargo ledger build nanosplus`), starts Speculos with the seed below, and
//! runs this file with `--ignored --test-threads=1`. Every test is
//! `#[ignore]`d so that a plain `cargo test` on a machine without Speculos
//! reports them skipped rather than failing on a refused connection.
//!
//! What these check that the host tests cannot:
//!
//! - that the device's own SLIP-0010 derivation (a syscall) yields the chain
//!   key the desktop wallet derives from the same phrase — asserted by
//!   deriving it here, independently, and comparing the device's public key;
//! - that paging survives the transport;
//! - that the review screens really gate signing: approve gives the exact
//!   deterministic signature the library produces, reject gives `0x6985`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use app_maya2c::apdu::{CLA, INS_GET_PAGE, MAX_APDU_PAYLOAD, Pages};
use app_maya2c::suite::{self, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use hmac::{Hmac, Mac};
use sha2::Sha512;

/// Speculos' APDU and REST ports, as the script starts it.
const APDU: &str = "127.0.0.1:9999";
const API: &str = "127.0.0.1:5000";

/// The phrase Speculos is started with (`--seed`): the BIP-39 test mnemonic.
pub const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                            abandon abandon abandon about";

const INS_GET_PUBLIC_KEY: u8 = 0x02;
const INS_SIGN: u8 = 0x04;
const INS_DISPLAY_ADDRESS: u8 = 0x06;

fn exchange(frame: &[u8]) -> (Vec<u8>, u16) {
    let mut stream = TcpStream::connect(APDU).expect("speculos APDU port");
    let mut out = u32::try_from(frame.len())
        .expect("len")
        .to_be_bytes()
        .to_vec();
    out.extend_from_slice(frame);
    stream.write_all(&out).expect("send");
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).expect("reply length");
    // Speculos' reply length excludes the status word, which follows.
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize + 2];
    stream.read_exact(&mut body).expect("reply");
    let status = u16::from_be_bytes([body[body.len() - 2], body[body.len() - 1]]);
    body.truncate(body.len() - 2);
    (body, status)
}

fn frame(ins: u8, p1: u8, p2: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![CLA, ins, p1, p2, u8::try_from(payload.len()).expect("≤255")];
    out.extend_from_slice(payload);
    out
}

fn path(account: u32, index: u32) -> Vec<u8> {
    let mut out = vec![5u8];
    for c in [44, 7331, account, 0, index] {
        out.extend_from_slice(&(c | 0x8000_0000u32).to_be_bytes());
    }
    out
}

/// A response of `len` bytes: page 0 already in hand, the rest by `GET_PAGE`.
fn collect(first: Vec<u8>, len: usize) -> Vec<u8> {
    let mut all = first;
    for page in 1..Pages::count_for(len) {
        let (bytes, status) = exchange(&frame(
            INS_GET_PAGE,
            0,
            u8::try_from(page).expect("page"),
            &[],
        ));
        assert_eq!(status, 0x9000, "page {page}");
        all.extend_from_slice(&bytes);
    }
    assert_eq!(all.len(), len);
    all
}

// --- the wallet's derivation, independently -------------------------------

fn hmac512(key: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    let mut mac = Hmac::<Sha512>::new_from_slice(key).expect("key");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// BIP-39 seed: PBKDF2-HMAC-SHA512, 2,048 rounds, salt "mnemonic".
fn bip39_seed(mnemonic: &str) -> [u8; 64] {
    let mut seed = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<Sha512>(mnemonic.as_bytes(), b"mnemonic", 2048, &mut seed);
    seed
}

/// SLIP-0010 ed25519, hardened indices only.
fn slip10(seed: &[u8], path: &[u32]) -> [u8; 32] {
    let i = hmac512(b"ed25519 seed", &[seed]);
    let (mut key, mut chain) = ([0u8; 32], [0u8; 32]);
    key.copy_from_slice(&i[..32]);
    chain.copy_from_slice(&i[32..]);
    for index in path {
        let i = hmac512(&chain, &[&[0], &key, &(index | 0x8000_0000).to_be_bytes()]);
        key.copy_from_slice(&i[..32]);
        chain.copy_from_slice(&i[32..]);
    }
    key
}

fn expected_keys(account: u32, index: u32) -> suite::Keypair {
    let chain_key = slip10(&bip39_seed(MNEMONIC), &[44, 7331, account, 0, index]);
    suite::keypair_from_chain_key(&chain_key)
}

#[test]
fn the_host_derivation_matches_published_vectors() {
    // BIP-39 (Trezor vectors) and SLIP-0010 (spec test vector 1): the host
    // side of the comparison must itself be right.
    assert_eq!(
        hex::encode(bip39_seed(MNEMONIC)),
        "5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc1\
         9a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4"
    );
    let seed = hex::decode("000102030405060708090a0b0c0d0e0f").expect("hex");
    assert_eq!(
        hex::encode(slip10(&seed, &[])),
        "2b4be7f19ee27bbf30c667b642d5f4aa69fd169872f8fc3059c08ebae2eb19e7"
    );
    assert_eq!(
        hex::encode(slip10(&seed, &[0])),
        "68e0fe46dfb67e368c75379acec591dad19df3cde26e63b93a8e704f1dade7a3"
    );
}

// --- driving the screens --------------------------------------------------

fn http(method: &str, route: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(API).expect("speculos API port");
    let request = format!(
        "{method} {route} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("request");
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    reply
}

fn screen_text() -> String {
    http("GET", "/events?currentscreenonly=true", "")
}

fn press(button: &str) {
    http(
        "POST",
        &format!("/button/{button}"),
        r#"{"action":"press-and-release"}"#,
    );
    std::thread::sleep(Duration::from_millis(150));
}

/// Walks right through the screens until one names `target`, then presses
/// both buttons on it. Runs beside a blocking APDU call; stops when `done`.
fn drive(target: &'static [&'static str], done: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for _ in 0..200 {
            if done.load(Ordering::SeqCst) {
                return;
            }
            let text = screen_text();
            if target.iter().any(|t| text.contains(t)) {
                press("both");
            } else {
                press("right");
            }
        }
    })
}

fn with_screens<T>(target: &'static [&'static str], call: impl FnOnce() -> T) -> T {
    let done = Arc::new(AtomicBool::new(false));
    let driver = drive(target, Arc::clone(&done));
    let result = call();
    done.store(true, Ordering::SeqCst);
    driver.join().expect("driver");
    result
}

const APPROVE: &[&str] = &["Sign transfer", "Confirm", "Approve"];
const REJECT: &[&str] = &["Reject"];

// --- the commands ---------------------------------------------------------

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn the_device_derives_the_wallets_key_from_the_same_phrase() {
    let (first, status) = exchange(&frame(INS_GET_PUBLIC_KEY, 0, 0, &path(0, 0)));
    assert_eq!(status, 0x9000);
    assert_eq!(first.len(), MAX_APDU_PAYLOAD);
    let key = collect(first, PUBLIC_KEY_LEN);
    let (public, _) = expected_keys(0, 0);
    assert_eq!(
        key,
        public.to_vec(),
        "device key ≠ SLIP-0010 → suite 0x10 key"
    );

    // Another index is another key.
    let (other, _) = exchange(&frame(INS_GET_PUBLIC_KEY, 0, 0, &path(0, 1)));
    assert_ne!(other, key[..MAX_APDU_PAYLOAD].to_vec());
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn a_foreign_coin_type_is_refused_by_the_device() {
    let mut bitcoin = vec![5u8];
    for c in [44u32, 0, 0, 0, 0] {
        bitcoin.extend_from_slice(&(c | 0x8000_0000).to_be_bytes());
    }
    let (_, status) = exchange(&frame(INS_GET_PUBLIC_KEY, 0, 0, &bitcoin));
    assert_eq!(status, 0x6A80);
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn a_page_past_the_end_is_refused() {
    let (_, status) = exchange(&frame(INS_GET_PUBLIC_KEY, 0, 0, &path(0, 0)));
    assert_eq!(status, 0x9000);
    let pages = u8::try_from(Pages::count_for(PUBLIC_KEY_LEN)).expect("pages");
    let (_, status) = exchange(&frame(INS_GET_PAGE, 0, pages, &[]));
    assert_eq!(status, 0x6A86);
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn the_address_is_shown_and_returned_once_confirmed() {
    let (address, status) = with_screens(APPROVE, || {
        exchange(&frame(INS_DISPLAY_ADDRESS, 0, 0, &path(0, 0)))
    });
    assert_eq!(status, 0x9000);
    let (public, _) = expected_keys(0, 0);
    assert_eq!(address, suite::address(&public).to_vec());
}

/// A v7 transfer's signing bytes for the device's own key, built from the
/// node's fixture with the key swapped in (same layout, same outputs).
fn transfer_for(public: &[u8; PUBLIC_KEY_LEN]) -> Vec<u8> {
    let text = include_str!("fixtures/suite-0x10-transfer.txt");
    let bytes = |name: &str| {
        hex::decode(
            text.lines()
                .find_map(|l| l.strip_prefix(&format!("{name}=")))
                .expect(name)
                .trim(),
        )
        .expect("hex")
    };
    let mut signing = bytes("signing_bytes");
    let fixture_key = bytes("public_key");
    let at = signing
        .windows(PUBLIC_KEY_LEN)
        .position(|w| w == fixture_key.as_slice())
        .expect("key inside the signing bytes");
    signing[at..at + PUBLIC_KEY_LEN].copy_from_slice(public);
    signing
}

/// Streams `path ‖ signing` as SIGN_TRANSACTION chunks; returns the last reply.
fn sign_on_device(signing: &[u8]) -> (Vec<u8>, u16) {
    let chunks: Vec<&[u8]> = Pages::new(signing).collect();
    let (_, status) = exchange(&frame(INS_SIGN, 0x00, 0, &path(0, 0)));
    assert_eq!(status, 0x9000, "path chunk");
    for (i, chunk) in chunks.iter().enumerate().take(chunks.len() - 1) {
        let (_, status) = exchange(&frame(
            INS_SIGN,
            0x01,
            u8::try_from(i + 1).expect("idx"),
            chunk,
        ));
        assert_eq!(status, 0x9000, "chunk {}", i + 1);
    }
    let last = u8::try_from(chunks.len()).expect("idx");
    exchange(&frame(INS_SIGN, 0x02, last, chunks[chunks.len() - 1]))
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn an_approved_transfer_is_signed_to_the_librarys_exact_bytes() {
    let (public, secret) = expected_keys(0, 0);
    let signing = transfer_for(&public);
    let (first, status) = with_screens(APPROVE, || sign_on_device(&signing));
    assert_eq!(status, 0x9000);
    let signature = collect(first, SIGNATURE_LEN);
    let expected = suite::sign(&secret, &signing).expect("host signature");
    assert_eq!(signature, expected.to_vec(), "deterministic: device = host");
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn a_rejected_transfer_is_not_signed() {
    let (public, _) = expected_keys(0, 0);
    let signing = transfer_for(&public);
    let (reply, status) = with_screens(REJECT, || sign_on_device(&signing));
    assert_eq!(status, 0x6985);
    assert!(reply.is_empty());
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn another_keys_transaction_is_refused_before_any_screen() {
    let (other, _) = expected_keys(0, 9);
    let (_, status) = sign_on_device(&transfer_for(&other));
    assert_eq!(status, 0x6A94, "ForeignKey");
}

#[test]
#[ignore = "needs Speculos: scripts/ledger_speculos.sh"]
fn an_interrupted_sequence_leaves_the_device_recoverable() {
    let (_, status) = exchange(&frame(INS_SIGN, 0x00, 0, &path(0, 0)));
    assert_eq!(status, 0x9000, "first chunk accepted");
    let (_, status) = exchange(&frame(INS_SIGN, 0x01, 2, &[0u8; 16]));
    assert_eq!(status, 0x6985, "out-of-order chunk refused");
    let (_, status) = exchange(&frame(INS_GET_PUBLIC_KEY, 0, 0, &path(0, 0)));
    assert_eq!(status, 0x9000, "the device recovered without a reconnect");
}
