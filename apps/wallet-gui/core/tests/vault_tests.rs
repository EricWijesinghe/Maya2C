//! Sealed-seed storage, including the real OS keychain.
//!
//! Most tests use [`MemoryStore`] — a test that writes to the user's actual
//! Credential Manager and leaves entries behind is a bad neighbour. One test
//! does exercise the real keychain, because a `SecretStore` implementation that
//! has never touched the OS is an assumption rather than a verified backend.
//! It uses a unique name and deletes what it wrote.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_wallet_core::error::WalletError;
use maya_wallet_core::hd::SEED_LEN;
use maya_wallet_core::vault::{MemoryStore, OsKeychain, SecretStore, Vault, seal, unseal};

fn seed(byte: u8) -> [u8; SEED_LEN] {
    [byte; SEED_LEN]
}

// ---------------------------------------------------------------------------
// envelope
// ---------------------------------------------------------------------------

#[test]
fn a_sealed_seed_round_trips() {
    let original = seed(7);
    let envelope = seal(&original, "correct horse battery staple").expect("seal");
    let recovered = unseal(&envelope, "correct horse battery staple").expect("unseal");

    assert_eq!(recovered.as_ref(), &original);
}

#[test]
fn the_envelope_has_the_documented_layout() {
    let envelope = seal(&seed(1), "pw").expect("seal");
    assert_eq!(&envelope[..8], b"MAYAVLT1");
    assert_eq!(envelope[8], 1);
    // header + seed + Poly1305 tag
    assert_eq!(envelope.len(), 8 + 1 + 16 + 12 + SEED_LEN + 16);
}

#[test]
fn the_seed_never_appears_in_the_envelope() {
    // A distinctive seed, so a plaintext leak would be unmistakable.
    let original = seed(0xAB);
    let envelope = seal(&original, "pw").expect("seal");

    assert!(
        !envelope
            .windows(SEED_LEN)
            .any(|window| window == original.as_slice()),
        "the plaintext seed is present in the sealed envelope"
    );
}

#[test]
fn a_wrong_passphrase_does_not_unlock() {
    let envelope = seal(&seed(3), "right").expect("seal");
    assert_eq!(unseal(&envelope, "wrong").err(), Some(WalletError::Unlock));
}

#[test]
fn a_tampered_envelope_is_rejected() {
    let mut envelope = seal(&seed(3), "pw").expect("seal");
    // Flip a bit in the ciphertext; Poly1305 must catch it.
    let last = envelope.len() - 8;
    envelope[last] ^= 0x01;

    assert_eq!(unseal(&envelope, "pw").err(), Some(WalletError::Unlock));
}

#[test]
fn a_tampered_salt_is_rejected() {
    let mut envelope = seal(&seed(3), "pw").expect("seal");
    // A changed salt derives a different key, so authentication fails.
    envelope[10] ^= 0xFF;
    assert_eq!(unseal(&envelope, "pw").err(), Some(WalletError::Unlock));
}

#[test]
fn each_sealing_uses_fresh_randomness() {
    let original = seed(5);
    let first = seal(&original, "pw").expect("seal");
    let second = seal(&original, "pw").expect("seal");

    // Same seed, same passphrase, different bytes: a reused nonce under
    // ChaCha20 would leak the keystream.
    assert_ne!(first, second);
    assert_ne!(first[9..25], second[9..25], "salt was reused");
    assert_ne!(first[25..37], second[25..37], "nonce was reused");
}

#[test]
fn an_empty_passphrase_is_refused() {
    assert_eq!(seal(&seed(1), "").err(), Some(WalletError::EmptyPassphrase));
}

#[test]
fn a_malformed_envelope_is_rejected() {
    assert!(matches!(
        unseal(b"too short", "pw"),
        Err(WalletError::Format(_))
    ));

    let mut wrong_magic = seal(&seed(1), "pw").expect("seal");
    wrong_magic[0] = b'X';
    assert!(matches!(
        unseal(&wrong_magic, "pw"),
        Err(WalletError::Format(_))
    ));

    let mut wrong_version = seal(&seed(1), "pw").expect("seal");
    wrong_version[8] = 99;
    assert!(matches!(
        unseal(&wrong_version, "pw"),
        Err(WalletError::Format(_))
    ));
}

// ---------------------------------------------------------------------------
// vault
// ---------------------------------------------------------------------------

#[test]
fn a_stored_wallet_unlocks() {
    let vault = Vault::new(MemoryStore::new());
    let original = seed(9);

    assert!(!vault.exists("primary").expect("exists"));
    vault.store("primary", &original, "pw").expect("store");
    assert!(vault.exists("primary").expect("exists"));

    let recovered = vault.unlock("primary", "pw").expect("unlock");
    assert_eq!(recovered.as_ref(), &original);
}

#[test]
fn storing_over_an_existing_wallet_is_refused() {
    let vault = Vault::new(MemoryStore::new());
    vault.store("primary", &seed(1), "pw").expect("store");

    // The stored seed may be the only copy of a key; overwriting it silently
    // would destroy funds.
    assert_eq!(
        vault.store("primary", &seed(2), "pw").err(),
        Some(WalletError::WalletExists("primary".to_string()))
    );

    // The original survives.
    assert_eq!(
        vault.unlock("primary", "pw").expect("unlock").as_ref(),
        &seed(1)
    );
}

#[test]
fn replacing_a_wallet_is_explicit() {
    let vault = Vault::new(MemoryStore::new());
    vault.store("primary", &seed(1), "pw").expect("store");
    vault.replace("primary", &seed(2), "pw").expect("replace");

    assert_eq!(
        vault.unlock("primary", "pw").expect("unlock").as_ref(),
        &seed(2)
    );
}

#[test]
fn unlocking_an_absent_wallet_says_so() {
    let vault = Vault::new(MemoryStore::new());
    assert_eq!(
        vault.unlock("missing", "pw").err(),
        Some(WalletError::NoWallet("missing".to_string()))
    );
}

#[test]
fn forgetting_a_wallet_removes_it() {
    let vault = Vault::new(MemoryStore::new());
    vault.store("primary", &seed(1), "pw").expect("store");
    vault.forget("primary").expect("forget");

    assert!(!vault.exists("primary").expect("exists"));
    // Forgetting something absent is not an error.
    vault.forget("primary").expect("idempotent");
}

#[test]
fn several_wallets_coexist() {
    let vault = Vault::new(MemoryStore::new());
    vault.store("personal", &seed(1), "one").expect("store");
    vault.store("savings", &seed(2), "two").expect("store");

    assert_eq!(
        vault.unlock("personal", "one").expect("unlock").as_ref(),
        &seed(1)
    );
    assert_eq!(
        vault.unlock("savings", "two").expect("unlock").as_ref(),
        &seed(2)
    );
    // Each wallet's passphrase is its own.
    assert!(vault.unlock("savings", "one").is_err());
}

// ---------------------------------------------------------------------------
// the real OS keychain
// ---------------------------------------------------------------------------

#[test]
fn the_os_keychain_round_trips() {
    // Exercises the actual platform backend — Windows Credential Manager on
    // this machine. A SecretStore that has never touched the OS is an
    // assumption, not a verified integration.
    let keychain = OsKeychain::new();
    let name = format!(
        "maya-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );

    // Absent before anything is written.
    match keychain.get(&name) {
        Ok(None) => {}
        Ok(Some(_)) => panic!("a freshly generated name already had a secret"),
        Err(error) => {
            // No usable keychain on this machine (a headless CI box, say).
            // Skipping beats a red test that says nothing about the code.
            eprintln!("skipping: OS keychain unavailable ({error})");
            return;
        }
    }

    let envelope = seal(&seed(0x5A), "keychain test passphrase").expect("seal");

    keychain.put(&name, &envelope).expect("write to keychain");
    let read_back = keychain.get(&name).expect("read").expect("present");
    assert_eq!(read_back, envelope);

    let recovered = unseal(&read_back, "keychain test passphrase").expect("unseal");
    assert_eq!(recovered.as_ref(), &seed(0x5A));

    // Clean up: leaving entries in a user's real keychain is antisocial.
    keychain.delete(&name).expect("delete");
    assert_eq!(keychain.get(&name).expect("read"), None);
}

#[test]
fn a_vault_over_the_os_keychain_works_end_to_end() {
    let name = format!("maya-vault-test-{}", std::process::id());
    let vault = Vault::new(OsKeychain::new());

    if vault.exists(&name).is_err() {
        eprintln!("skipping: OS keychain unavailable");
        return;
    }

    // A previous aborted run may have left this behind.
    let _ = vault.forget(&name);

    vault
        .replace(&name, &seed(0x42), "vault pass")
        .expect("store");
    let recovered = vault.unlock(&name, "vault pass").expect("unlock");
    assert_eq!(recovered.as_ref(), &seed(0x42));

    assert_eq!(
        vault.unlock(&name, "wrong").err(),
        Some(WalletError::Unlock)
    );

    vault.forget(&name).expect("forget");
}
