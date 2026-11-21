//! ADR-049 sealed validator keys, red-teamed through the public API only.
//!
//! The unit tests beside `sealed.rs` cover a wrong passphrase and tampering
//! with the public key, salt, KDF passes and ciphertext. These are the attacks
//! left over: relabelling a file as another kind, a format or KDF swap, nonce
//! tampering, transplanting one file's ciphertext into another, truncation, and
//! inputs that are not a sealed file at all. Each must fail closed, never
//! open as some other secret and never panic.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_signer::keystore::KeystoreError;
use maya_signer::sealed::{self, VALIDATOR_KEY_KIND};
use serde_json::Value;

const PASS: &[u8] = b"correct horse battery staple";

fn field(text: &str, name: &str) -> Value {
    serde_json::from_str::<Value>(text).unwrap()[name].clone()
}

fn with(text: &str, name: &str, value: Value) -> String {
    let mut json: Value = serde_json::from_str(text).unwrap();
    json[name] = value;
    json.to_string()
}

#[test]
fn relabelling_a_file_as_another_kind_fails_the_tag() {
    // The kind check alone would pass once the label is edited to match; only
    // the kind being in the associated data stops a wallet key being opened as
    // a validator key.
    let text = sealed::seal(b"wallet secret", b"pk", "maya2c-wallet-seed", PASS).unwrap();
    let relabelled = with(&text, "kind", VALIDATOR_KEY_KIND.into());
    assert!(matches!(
        sealed::open(&relabelled, VALIDATOR_KEY_KIND, PASS),
        Err(KeystoreError::Decrypt)
    ));
}

#[test]
fn a_format_or_kdf_swap_is_refused_as_a_format_error() {
    let text = sealed::seal(b"s", b"p", VALIDATOR_KEY_KIND, PASS).unwrap();
    for (name, value) in [
        ("format", Value::from(1)),
        ("format", Value::from(3)),
        ("kdf", Value::from("argon2i")),
        ("kdf", Value::from("scrypt")),
    ] {
        assert!(
            matches!(
                sealed::open(&with(&text, name, value.clone()), VALIDATOR_KEY_KIND, PASS),
                Err(KeystoreError::Format(_))
            ),
            "{name} = {value}"
        );
    }
}

#[test]
fn a_tampered_or_resized_nonce_never_opens() {
    let text = sealed::seal(b"s", b"p", VALIDATOR_KEY_KIND, PASS).unwrap();
    let nonce = field(&text, "nonce").as_str().unwrap().to_string();
    let flipped = format!(
        "{}{}",
        if nonce.starts_with('0') { "1" } else { "0" },
        &nonce[1..]
    );
    assert!(matches!(
        sealed::open(
            &with(&text, "nonce", flipped.into()),
            VALIDATOR_KEY_KIND,
            PASS
        ),
        Err(KeystoreError::Decrypt)
    ));
    for resized in [&nonce[..22], &format!("{nonce}00")[..]] {
        assert!(matches!(
            sealed::open(
                &with(&text, "nonce", resized.into()),
                VALIDATOR_KEY_KIND,
                PASS
            ),
            Err(KeystoreError::Format(_))
        ));
    }
}

#[test]
fn a_ciphertext_transplanted_between_files_does_not_open() {
    // Same passphrase, same kind, same public key: only the salt differs, and
    // it is both in the key derivation and in the associated data.
    let a = sealed::seal(b"secret A", b"pk", VALIDATOR_KEY_KIND, PASS).unwrap();
    let b = sealed::seal(b"secret B", b"pk", VALIDATOR_KEY_KIND, PASS).unwrap();
    let mut forged = with(&b, "ciphertext", field(&a, "ciphertext"));
    forged = with(&forged, "nonce", field(&a, "nonce"));
    assert!(matches!(
        sealed::open(&forged, VALIDATOR_KEY_KIND, PASS),
        Err(KeystoreError::Decrypt)
    ));
}

#[test]
fn sealing_twice_never_reuses_a_salt_or_nonce() {
    let a = sealed::seal(b"same", b"pk", VALIDATOR_KEY_KIND, PASS).unwrap();
    let b = sealed::seal(b"same", b"pk", VALIDATOR_KEY_KIND, PASS).unwrap();
    for name in ["salt", "nonce", "ciphertext"] {
        assert_ne!(field(&a, name), field(&b, name), "{name} repeated");
    }
}

#[test]
fn a_truncated_ciphertext_fails_closed() {
    let text = sealed::seal(
        b"thirty-two bytes of seed material",
        b"p",
        VALIDATOR_KEY_KIND,
        PASS,
    )
    .unwrap();
    let ct = field(&text, "ciphertext").as_str().unwrap().to_string();
    // Shorter than the 16-byte tag, exactly the tag, and one byte short.
    for cut in ["", &ct[..16], &ct[..32], &ct[..ct.len() - 2]] {
        assert!(
            matches!(
                sealed::open(
                    &with(&text, "ciphertext", cut.into()),
                    VALIDATOR_KEY_KIND,
                    PASS
                ),
                Err(KeystoreError::Decrypt)
            ),
            "ciphertext cut to {} hex chars",
            cut.len()
        );
    }
}

#[test]
fn input_that_is_not_a_sealed_file_is_a_format_error() {
    for text in ["", "{}", "[]", "not json", "{\"format\":2}"] {
        assert!(
            matches!(
                sealed::open(text, VALIDATOR_KEY_KIND, PASS),
                Err(KeystoreError::Format(_))
            ),
            "{text:?}"
        );
    }
    // A legacy plaintext hex key is told apart before anyone tries to open it.
    assert!(!sealed::is_sealed("6db2190cacb1885f6db2190cacb1885f"));
    assert!(sealed::is_sealed("  \n{\"format\":2}"));
}

#[test]
fn the_passphrase_minimum_is_exact() {
    let short = vec![b'x'; sealed::MIN_PASSPHRASE_LEN - 1];
    let exact = vec![b'x'; sealed::MIN_PASSPHRASE_LEN];
    assert!(matches!(
        sealed::seal(b"s", b"p", VALIDATOR_KEY_KIND, &short),
        Err(KeystoreError::Format(_))
    ));
    let text = sealed::seal(b"s", b"p", VALIDATOR_KEY_KIND, &exact).unwrap();
    assert_eq!(
        sealed::open(&text, VALIDATOR_KEY_KIND, &exact)
            .unwrap()
            .0
            .as_slice(),
        b"s"
    );
}
