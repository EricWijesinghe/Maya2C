//! The Ledger app's constants and address derivation, against the node's.
//!
//! # Why this lives in the node's test suite
//!
//! `app-maya2c` cannot depend on `custom-l1-node` — that would pull RocksDB's
//! C++ into a Cortex-M binary — so it duplicates two constants and reimplements
//! address derivation. Duplication of values that decide *where funds land* is
//! exactly what drifts silently, and an independent implementation that
//! disagrees is indistinguishable from a correct one until somebody loses
//! money.
//!
//! So the comparison happens here, where both sides are reachable. This is the
//! same arrangement `tests/hybrid_parity_tests.rs` has with `sdk-wasm`, for the
//! same reason.
//!
//! # This caught a real bug
//!
//! The app's first derivation used `blake3::Hasher::new_derive_key(domain)`.
//! The node prefixes the domain into a plain hasher
//! (`src/crypto/hybrid.rs:216`). Those produce different digests from identical
//! inputs, and the failure mode is an address no key can spend from — with no
//! error anywhere. The mistake was found by reading the node rather than by
//! this test, but this is what stops it coming back.

use custom_l1_node::crypto::hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LENGTH, SLH_DSA_SIGNATURE_LENGTH, address_of,
    generate_signing_key,
};

/// Re-derived here rather than imported: `app-maya2c` targets `thumbv8m` and
/// cannot be a dependency of this crate. Keeping the *values* in one assertion
/// is the next best thing to sharing the code.
mod app {
    /// `apps/ledger-maya2c/src/derive.rs`.
    pub const COIN_TYPE: u32 = 7331;
    /// `apps/ledger-maya2c/src/derive.rs`.
    pub const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v3";

    /// `apps/ledger-maya2c/src/apdu.rs`.
    pub const HYBRID_PUBLIC_KEY_LEN: usize = 1984;
    /// `apps/ledger-maya2c/src/apdu.rs`.
    pub const HYBRID_SIGNATURE_LEN: usize = 11165;
    /// `apps/ledger-maya2c/src/apdu.rs`.
    pub const ML_DSA_SIGNATURE_LEN: usize = 3309;

    /// The app's derivation, copied verbatim from `address_from_public_key`.
    ///
    /// Copied rather than called, because the crate it lives in does not build
    /// for this target. A copy that drifted from the app would make this test
    /// pass while the device was wrong, which is why the app's version carries
    /// a comment pointing here.
    #[must_use]
    pub fn address_from_public_key(public_key: &[u8]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(ADDRESS_DOMAIN);
        hasher.update(public_key);
        *hasher.finalize().as_bytes()
    }
}

#[test]
fn the_apps_size_constants_match_the_nodes() {
    // Every one of these is hard-coded in the app's APDU layer, where the
    // chunking arithmetic depends on it. A signature length that disagreed by
    // one byte would page into a different number of responses.
    assert_eq!(app::HYBRID_PUBLIC_KEY_LEN, HYBRID_PUBLIC_KEY_LEN);
    assert_eq!(app::HYBRID_SIGNATURE_LEN, HYBRID_SIGNATURE_LENGTH);
    // The lattice half is derived rather than imported: the node keeps its
    // ML-DSA length as a private alias, and the subtraction pins the split as
    // well as the total.
    assert_eq!(
        app::ML_DSA_SIGNATURE_LEN,
        HYBRID_SIGNATURE_LENGTH - SLH_DSA_SIGNATURE_LENGTH
    );
}

#[test]
fn the_apps_coin_type_matches_the_wallets() {
    assert_eq!(
        app::COIN_TYPE,
        maya_wallet_core::hd::COIN_TYPE,
        "a device deriving on a different coin type produces keys the desktop \
         wallet cannot find"
    );
}

#[test]
fn the_apps_address_derivation_matches_the_node() {
    // The assertion this file exists for. Run over freshly generated keys
    // rather than a fixed vector, so it covers the derivation rather than one
    // pair of inputs somebody chose.
    for _ in 0..4 {
        let key = generate_signing_key().expect("keygen");
        let public = key.public_key();

        let mut encoded = Vec::new();
        public.encode_into(&mut encoded);
        assert_eq!(
            encoded.len(),
            HYBRID_PUBLIC_KEY_LEN,
            "the app pages the encoded key at this length"
        );

        assert_eq!(
            app::address_from_public_key(&encoded),
            address_of(&public),
            "the device would show an address the chain does not agree with"
        );
    }
}

#[test]
fn a_keyed_derivation_would_not_have_matched() {
    // Pinning the specific mistake. `new_derive_key` is the obvious-looking
    // call and it is wrong here; without this, a future edit that "tidies" the
    // prefix into a keyed context would pass every other test in this file
    // while silently changing every address on the chain.
    let key = generate_signing_key().expect("keygen");
    let public = key.public_key();
    let mut encoded = Vec::new();
    public.encode_into(&mut encoded);

    let mut keyed =
        blake3::Hasher::new_derive_key(core::str::from_utf8(app::ADDRESS_DOMAIN).expect("ascii"));
    keyed.update(&encoded);

    assert_ne!(
        *keyed.finalize().as_bytes(),
        address_of(&public),
        "if these ever agree, this test is no longer pinning anything"
    );
}
