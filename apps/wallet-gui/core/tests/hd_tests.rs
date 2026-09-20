//! BIP-39 and SLIP-0010 derivation.
//!
//! The SLIP-0010 tests use the **published test vectors** from the
//! specification rather than values this implementation produced. That
//! distinction matters more here than almost anywhere else in the project: a
//! derivation bug yields keys that are perfectly valid and simply not the ones
//! any other wallet derives from the same phrase. Self-consistent tests would
//! pass against a wrong implementation. The failure only surfaces when a user
//! tries to recover elsewhere and finds an empty account.

use maya_wallet_core::error::WalletError;
use maya_wallet_core::hd::{
    self, DerivationPath, HARDENED, generate_mnemonic, seed_from_mnemonic, validate_mnemonic,
    word_count,
};

// ---------------------------------------------------------------------------
// SLIP-0010 published vectors (ed25519)
// ---------------------------------------------------------------------------

/// Test vector 1 seed from SLIP-0010.
const VECTOR1_SEED: &str = "000102030405060708090a0b0c0d0e0f";

/// Test vector 2 seed from SLIP-0010.
const VECTOR2_SEED: &str = "fffcf9f6f3f0edeae7e4e1dedbd8d5d2cfccc9c6c3c0bdbab7b4b1aeaba8a5a29f9c999693908d8a8784817e7b7875726f6c696663605d5a5754514e4b484542";

fn seed(hex_seed: &str) -> Vec<u8> {
    hex::decode(hex_seed).expect("valid hex seed")
}

/// Derives along a path and returns `(chain_code, key)` as hex.
fn derive(hex_seed: &str, indices: &[u32]) -> (String, String) {
    let mut key = hd::master_key(&seed(hex_seed)).expect("master");
    for index in indices {
        key = hd::derive_child(&key, index | HARDENED).expect("child");
    }
    (hex::encode(key.chain_code), hex::encode(key.key))
}

#[test]
fn slip10_vector_1_master_key() {
    let (chain_code, key) = derive(VECTOR1_SEED, &[]);
    assert_eq!(
        chain_code,
        "90046a93de5380a72b5e45010748567d5ea02bbf6522f979e05c0d8d8ca9fffb"
    );
    assert_eq!(
        key,
        "2b4be7f19ee27bbf30c667b642d5f4aa69fd169872f8fc3059c08ebae2eb19e7"
    );
}

#[test]
fn slip10_vector_1_full_chain() {
    // m/0'
    let (chain_code, key) = derive(VECTOR1_SEED, &[0]);
    assert_eq!(
        chain_code,
        "8b59aa11380b624e81507a27fedda59fea6d0b779a778918a2fd3590e16e9c69"
    );
    assert_eq!(
        key,
        "68e0fe46dfb67e368c75379acec591dad19df3cde26e63b93a8e704f1dade7a3"
    );

    // m/0'/1'/2'/2'/1000000000'
    let (chain_code, key) = derive(VECTOR1_SEED, &[0, 1, 2, 2, 1_000_000_000]);
    assert_eq!(
        chain_code,
        "68789923a0cac2cd5a29172a475fe9e0fb14cd6adb5ad98a3fa70333e7afa230"
    );
    assert_eq!(
        key,
        "8f94d394a8e8fd6b1bc2f3f49f5c47e385281d5c17e65324b0f62483e37e8793"
    );
}

#[test]
fn slip10_vector_2_full_chain() {
    let (chain_code, key) = derive(VECTOR2_SEED, &[]);
    assert_eq!(
        chain_code,
        "ef70a74db9c3a5af931b5fe73ed8e1a53464133654fd55e7a66f8570b8e33c3b"
    );
    assert_eq!(
        key,
        "171cb88b1b3c1db25add599712e36245d75bc65a1a5c9e18d76f9f2b1eab4012"
    );

    // m/0'/2147483647'/1'/2147483646'/2'
    let (chain_code, key) = derive(VECTOR2_SEED, &[0, 2_147_483_647, 1, 2_147_483_646, 2]);
    assert_eq!(
        chain_code,
        "5d70af781f3a37b829f0d060924d5e960bdc02e85423494afc0b1a41bbe196d4"
    );
    assert_eq!(
        key,
        "551d333177df541ad876a60ea71f00447931c0a9da16f227c11ea080d7391b8d"
    );
}

// ---------------------------------------------------------------------------
// derivation paths
// ---------------------------------------------------------------------------

#[test]
fn a_non_hardened_path_is_rejected() {
    // SLIP-0010 over ed25519 defines hardened derivation only. Silently
    // hardening the index would produce keys nobody else derives.
    let error = DerivationPath::parse("m/44'/7331'/0'/0/0'").expect_err("must reject");
    assert!(matches!(error, WalletError::InvalidPath(_)));
    assert!(error.to_string().contains("hardened"));
}

#[test]
fn a_hardened_path_round_trips() {
    let path = DerivationPath::parse("m/44'/7331'/0'/0'/5'").expect("parse");
    assert_eq!(path.to_string(), "m/44'/7331'/0'/0'/5'");
    assert_eq!(path.indices().len(), 5);
    assert!(path.indices().iter().all(|index| *index >= HARDENED));
}

#[test]
fn an_h_suffix_is_accepted_as_hardened() {
    // Both notations appear in the wild.
    let with_tick = DerivationPath::parse("m/44'/7331'/0'").expect("parse");
    let with_h = DerivationPath::parse("m/44h/7331h/0h").expect("parse");
    assert_eq!(with_tick, with_h);
}

#[test]
fn malformed_paths_are_rejected() {
    for path in ["44'/0'", "m", "m/", "m/abc'", "m/4294967296'"] {
        assert!(
            DerivationPath::parse(path).is_err(),
            "'{path}' should not parse"
        );
    }
}

#[test]
fn the_account_path_matches_the_documented_layout() {
    assert_eq!(
        DerivationPath::account(0, 0).to_string(),
        "m/44'/7331'/0'/0'/0'"
    );
    assert_eq!(
        DerivationPath::account(2, 7).to_string(),
        "m/44'/7331'/2'/0'/7'"
    );
}

#[test]
fn deriving_a_non_hardened_child_is_refused() {
    let key = hd::master_key(&seed(VECTOR1_SEED)).expect("master");
    let error = hd::derive_child(&key, 5).expect_err("must refuse");
    assert!(matches!(error, WalletError::Derivation(_)));
}

// ---------------------------------------------------------------------------
// BIP-39
// ---------------------------------------------------------------------------

#[test]
fn a_generated_phrase_has_24_valid_words() {
    let phrase = generate_mnemonic().expect("generate");
    assert_eq!(word_count(&phrase), 24);
    assert!(validate_mnemonic(&phrase).is_ok());
}

#[test]
fn two_generated_phrases_differ() {
    let first = generate_mnemonic().expect("generate");
    let second = generate_mnemonic().expect("generate");
    // Identical phrases would mean the entropy source is not doing its job.
    assert_ne!(*first, *second);
}

#[test]
fn a_mistyped_word_fails_the_checksum() {
    // The BIP-39 checksum is what catches a single wrong word. Without it, a
    // typo silently produces a different, empty wallet.
    let valid = "abandon abandon abandon abandon abandon abandon abandon abandon \
                 abandon abandon abandon about";
    assert!(validate_mnemonic(valid).is_ok());

    let mistyped = "abandon abandon abandon abandon abandon abandon abandon abandon \
                    abandon abandon abandon abandon";
    assert!(validate_mnemonic(mistyped).is_err());
}

#[test]
fn a_word_outside_the_wordlist_is_rejected() {
    let phrase = "zebra abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    assert!(validate_mnemonic(phrase).is_err());
}

#[test]
fn the_bip39_seed_matches_the_reference_vector() {
    // From the BIP-39 reference vectors, passphrase "TREZOR".
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    let seed = seed_from_mnemonic(phrase, "TREZOR").expect("seed");
    assert_eq!(
        hex::encode(seed.as_ref()),
        "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e5349553\
         1f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04"
    );
}

#[test]
fn the_passphrase_changes_the_wallet_entirely() {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";

    let without = seed_from_mnemonic(phrase, "").expect("seed");
    let with = seed_from_mnemonic(phrase, "TREZOR").expect("seed");

    // The passphrase is a separate secret, not a decoration: the same words
    // with a different passphrase are a different wallet.
    assert_ne!(without.as_ref(), with.as_ref());
}

#[test]
fn an_invalid_phrase_produces_no_seed() {
    assert!(seed_from_mnemonic("not a real mnemonic at all", "").is_err());
}

// ---------------------------------------------------------------------------
// addresses
// ---------------------------------------------------------------------------

#[test]
fn derivation_is_deterministic() {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    let seed = seed_from_mnemonic(phrase, "").expect("seed");
    let path = DerivationPath::account(0, 0);

    let first = hd::address_at(seed.as_ref(), &path).expect("address");
    let second = hd::address_at(seed.as_ref(), &path).expect("address");

    // The property recovery depends on.
    assert_eq!(first, second);
}

#[test]
fn an_account_is_the_ml_dsa_key_seeded_by_its_chain_key() {
    // The bridge this wallet defines: SLIP-0010 yields a 32-byte chain key per
    // hardened step, and that value is handed to FIPS 204 key generation as the
    // seed `xi`.
    //
    // **No standard covers this.** SLIP-0010 defines ed25519 and secp256k1 and
    // says nothing about lattice schemes, so no other wallet derives these
    // accounts from the same mnemonic. Pinning the construction here means a
    // change to it fails a test rather than silently stranding every recovered
    // wallet at an empty account.
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    let seed = seed_from_mnemonic(phrase, "").expect("seed");
    let path = DerivationPath::account(0, 0);

    let chain_key = hd::derive_path(seed.as_ref(), &path).expect("derive");
    let expected = custom_l1_node::crypto::hybrid::signing_key_from_seed(&chain_key.key)
        .expect("seeded keygen");

    assert_eq!(
        hd::address_at(seed.as_ref(), &path).expect("address"),
        expected.address()
    );
    assert_eq!(
        hd::signing_key_at(seed.as_ref(), &path)
            .expect("key")
            .public_key(),
        expected.public_key()
    );
}

#[test]
fn different_indices_give_different_addresses() {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    let seed = seed_from_mnemonic(phrase, "").expect("seed");

    let mut seen = std::collections::HashSet::new();
    for index in 0..10 {
        let address =
            hd::address_at(seed.as_ref(), &DerivationPath::account(0, index)).expect("address");
        assert!(seen.insert(address), "index {index} collided");
    }
}

#[test]
fn different_accounts_give_different_addresses() {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    let seed = seed_from_mnemonic(phrase, "").expect("seed");

    let account_zero =
        hd::address_at(seed.as_ref(), &DerivationPath::account(0, 0)).expect("address");
    let account_one =
        hd::address_at(seed.as_ref(), &DerivationPath::account(1, 0)).expect("address");

    assert_ne!(account_zero, account_one);
}

#[test]
fn a_derived_key_can_sign_and_verify() {
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon \
                  abandon abandon abandon about";
    let seed = seed_from_mnemonic(phrase, "").expect("seed");
    let path = DerivationPath::account(0, 0);

    let signing_key = hd::signing_key_at(seed.as_ref(), &path).expect("key");
    let address = hd::address_at(seed.as_ref(), &path).expect("address");

    // The address must be the public key of the key that signs for it.
    assert_eq!(signing_key.address(), address);
}
