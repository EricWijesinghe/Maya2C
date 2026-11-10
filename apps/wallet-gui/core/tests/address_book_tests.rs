//! ADR-086 §6.2 and §7: the address book is sealed at rest, names are the
//! user's, and the poisoning defenses read it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;

use maya_clear_sign::Network;
use maya_wallet_core::address_book::{Activity, AddressBook, BookKey, Source, recent};
use maya_wallet_core::error::WalletError;
use maya_wallet_core::payment::PaymentRequest;
use maya_wallet_core::vault::{MemoryStore, SecretStore};
use maya_wallet_core::wallet::Wallet;
use primitive_types::U256;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon about";

fn key() -> BookKey {
    Wallet::from_mnemonic(PHRASE, "").unwrap().address_book_key()
}

fn book_with(names: &[(&str, u8)]) -> AddressBook {
    let mut book = AddressBook::default();
    for (name, tag) in names {
        book.add([*tag; 32], Network::Testnet, name, "", Source::Manual, 1)
            .unwrap();
    }
    book
}

#[test]
fn the_book_is_encrypted_at_rest_and_opens_only_with_its_wallets_key() {
    let book = book_with(&[("Alice", 1), ("Bob", 2)]);
    let store = MemoryStore::new();
    book.save(&store, "main", &key()).unwrap();

    let raw = store.get("main.contacts").unwrap().unwrap();
    let as_text = String::from_utf8_lossy(&raw);
    assert!(!as_text.contains("Alice") && !as_text.contains("Bob"), "names in clear");
    assert!(!raw.windows(32).any(|w| w == [1u8; 32]), "address in clear");

    assert_eq!(AddressBook::load(&store, "main", &key()).unwrap(), book);

    let other = Wallet::from_mnemonic(PHRASE, "a different passphrase")
        .unwrap()
        .address_book_key();
    assert!(matches!(
        AddressBook::load(&store, "main", &other),
        Err(WalletError::Unlock)
    ));

    let mut tampered = raw.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(matches!(
        AddressBook::open(&tampered, &key()),
        Err(WalletError::Unlock)
    ));
    // Two seals of the same book differ: the nonce is fresh each time.
    assert_ne!(book.seal(&key()).unwrap(), book.seal(&key()).unwrap());
    // No book yet is an empty book, not an error.
    assert_eq!(
        AddressBook::load(&store, "nobody", &key()).unwrap(),
        AddressBook::default()
    );
}

#[test]
fn an_export_imports_into_the_same_wallet_and_skips_what_is_there() {
    let exported = book_with(&[("Alice", 1), ("Bob", 2)]).seal(&key()).unwrap();
    let mut here = book_with(&[("Alice", 1)]);
    assert_eq!(here.import(&exported, &key()).unwrap(), 1);
    assert_eq!(here.contacts().len(), 2);
    assert_eq!(here.import(&exported, &key()).unwrap(), 0);
    assert!(AddressBook::open(b"not a book", &key()).is_err());
}

#[test]
fn a_request_label_is_never_a_name() {
    // The label arrives in a request; the book takes only the typed name.
    let request = PaymentRequest::parse(&format!(
        "maya:{}?label=Alice%20(verified)",
        hex::encode([9u8; 32])
    ))
    .unwrap();
    let mut book = AddressBook::default();
    let typed = "Coffee shop on 5th";
    book.add([9; 32], Network::Testnet, typed, "", Source::Scanned, 1)
        .unwrap();
    let saved = book.find(&[9; 32]).unwrap();
    assert_eq!(saved.name, typed);
    assert_ne!(Some(saved.name.clone()), request.label);
}

#[test]
fn confusable_and_mixed_alphabet_names_warn_and_invisible_controls_are_removed() {
    let mut book = book_with(&[("Paypal", 1)]);
    // Cyrillic "а" in place of Latin "a".
    let check = book
        .add([2; 32], Network::Testnet, "P\u{0430}ypal", "", Source::Manual, 1)
        .unwrap();
    assert!(check.warnings.iter().any(|w| w.contains("mixes alphabets")), "{check:?}");
    assert!(check.warnings.iter().any(|w| w.contains("'Paypal'")), "{check:?}");
    // l and 1 and I read alike.
    let check = book
        .add([3; 32], Network::Testnet, "PaypaI", "", Source::Manual, 1)
        .unwrap();
    assert!(check.warnings.iter().any(|w| w.contains("'Paypal'")), "{check:?}");
    // A right-to-left override is stripped before the name is stored.
    let check = book
        .add([4; 32], Network::Testnet, "\u{202E}Carol", "", Source::Manual, 1)
        .unwrap();
    assert_eq!(check.name, "Carol");
    assert!(check.warnings.is_empty());
    // The same address twice is refused, naming the existing entry.
    let again = book.add([4; 32], Network::Testnet, "Dave", "", Source::Manual, 1);
    assert!(matches!(again, Err(WalletError::Contact(m)) if m.contains("Carol")));
}

#[test]
fn a_paste_is_checked_against_the_book_and_the_network() {
    let book = book_with(&[("Alice", 0x22)]);
    let alice = [0x22_u8; 32];
    let paid: BTreeSet<[u8; 32]> = [[0x33; 32]].into_iter().collect();

    let saved = book
        .check_paste(&maya_address::encode(Network::Testnet, &alice), Network::Testnet, &paid)
        .unwrap();
    assert_eq!(saved.contact.as_deref(), Some("Alice"));
    assert!(!saved.new_recipient);

    let mut poison = alice;
    poison[12] ^= 0x01;
    let look = book
        .check_paste(&hex::encode(poison), Network::Testnet, &paid)
        .unwrap();
    assert_eq!(look.look_alike_of.as_deref(), Some("Alice"));
    assert!(look.new_recipient, "a look-alike is also new: the interstitial shows");

    let paid_before = book
        .check_paste(&hex::encode([0x33; 32]), Network::Testnet, &paid)
        .unwrap();
    assert!(!paid_before.new_recipient);

    // A grouped display pasted whole still parses.
    let grouped = maya_clear_sign::grouped(Network::Testnet, &alice);
    assert!(book.check_paste(&grouped, Network::Testnet, &paid).is_ok());

    // A mainnet address on a testnet wallet is refused before the form fills.
    let mainnet = maya_address::encode(Network::Mainnet, &alice);
    assert!(matches!(
        book.check_paste(&mainnet, Network::Testnet, &paid),
        Err(WalletError::Address(_))
    ));
}

#[test]
fn dust_and_never_paid_incoming_transfers_stay_out_of_recent() {
    let book = book_with(&[("Alice", 1)]);
    let dust = U256::from(1_000);
    let paid = [5u8; 32];
    let activity = vec![
        // A poisoner's dust from a look-alike nobody ever paid.
        Activity { counterparty: [9; 32], amount: U256::from(1), incoming: true },
        // A large incoming payment from a stranger: still never paid.
        Activity { counterparty: [8; 32], amount: U256::from(1_000_000), incoming: true },
        // Dust from a saved contact: hidden too.
        Activity { counterparty: [1; 32], amount: U256::from(5), incoming: true },
        Activity { counterparty: [1; 32], amount: U256::from(5_000), incoming: true },
        Activity { counterparty: paid, amount: U256::from(10), incoming: false },
        Activity { counterparty: paid, amount: U256::from(2_000), incoming: true },
    ];
    assert_eq!(recent(&activity, &book, dust), vec![[1; 32], paid]);
}
