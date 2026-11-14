//! Review fix M2: the Windows Credential Manager caps a secret at 2,560 bytes
//! (`CRED_MAX_CREDENTIAL_BLOB_SIZE`), so the sealed book lives in a file in
//! the wallet's data directory. The key is still derived from the seed in the
//! keychain, so the file is useless without an unlocked wallet.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_clear_sign::Network;
use maya_wallet_core::address_book::{AddressBook, Source};
use maya_wallet_core::error::WalletError;
use maya_wallet_core::wallet::Wallet;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon about";
/// Windows `CRED_MAX_CREDENTIAL_BLOB_SIZE`, the keychain limit the file avoids.
const WINDOWS_CREDENTIAL_BLOB_MAX: usize = 2_560;

fn address(i: u16) -> [u8; 32] {
    let mut a = [0x5C_u8; 32];
    a[..2].copy_from_slice(&i.to_le_bytes());
    a[31] = u8::try_from(i % 251).unwrap();
    a
}

#[test]
fn a_book_of_150_contacts_round_trips_through_its_file() {
    let key = Wallet::from_mnemonic(PHRASE, "")
        .unwrap()
        .address_book_key();
    let mut book = AddressBook::default();
    for i in 0..150u16 {
        book.add(
            address(i),
            Network::Testnet,
            &format!("Contact number {i}"),
            "a note",
            Source::Manual,
            u64::from(i),
        )
        .unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.contacts");
    book.save_file(&path, &key).unwrap();

    let raw = std::fs::read(&path).unwrap();
    println!(
        "150 contacts seal to {} bytes; the Windows credential limit is {WINDOWS_CREDENTIAL_BLOB_MAX}",
        raw.len()
    );
    assert!(
        raw.len() > WINDOWS_CREDENTIAL_BLOB_MAX,
        "the file is needed at this size"
    );
    assert!(!String::from_utf8_lossy(&raw).contains("Contact number"));
    assert_eq!(AddressBook::load_file(&path, &key).unwrap(), book);

    // Nothing is left beside the file after an atomic replace.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");

    // No file yet is an empty book; the wrong key is refused.
    let missing = dir.path().join("none.contacts");
    assert_eq!(
        AddressBook::load_file(&missing, &key).unwrap(),
        AddressBook::default()
    );
    let other = Wallet::from_mnemonic(PHRASE, "other")
        .unwrap()
        .address_book_key();
    assert!(matches!(
        AddressBook::load_file(&path, &other),
        Err(WalletError::Unlock)
    ));
}

#[test]
fn an_address_is_stored_compactly_not_as_a_json_number_array() {
    let key = Wallet::from_mnemonic(PHRASE, "")
        .unwrap()
        .address_book_key();
    let mut book = AddressBook::default();
    book.add(address(1), Network::Testnet, "A", "", Source::Manual, 0)
        .unwrap();
    // 32 bytes as JSON numbers is ~100 bytes; as hex it is 64 plus quotes.
    let one = book.seal(&key).unwrap().len();
    println!("one contact seals to {one} bytes");
    assert!(one < 200, "{one}");
}
