//! Wallet derivation and transfer signing, for the browser-extension wallet.
//!
//! # Why this is a copy of the desktop wallet's derivation
//!
//! `apps/wallet-gui/core` derives accounts through `custom_l1_node`, which
//! links `RocksDB` and cannot target wasm (see the crate docs). So the same
//! chain is rebuilt here from the same primitives: BIP-39 seed (PBKDF2-SHA512,
//! 2048 rounds), SLIP-0010 hardened derivation on `m/44'/2992'/account'/0'/index'`,
//! the chain key split into an ML-DSA-65 and an SLH-DSA-SHA2-128s seed, and the
//! plain-transfer wire format (version 5).
//!
//! **A drift here is a wallet that shows an address nobody else derives, or a
//! transaction the chain rejects.** `tests/desktop_parity_tests.rs` compares
//! every step against `maya-wallet-core` and the node byte for byte, and
//! `spec/wallet-derivation-vectors.json` carries the result to the JavaScript
//! tests, so either side moving fails a test.
//!
//! Nothing here is new cryptography; it is the node's construction, copied.

use fips204::ml_dsa_65;
use fips204::traits::{KeyGen as _, SerDes as _, Signer as _};
use hmac::{Hmac, Mac as _};
use maya_amount::Amount;
use maya_clear_sign::{Context, Effect, Network, grouped, render, review, warning_text};
use maya_crypto_pq::sig as slh;
use sha2::Sha512;
use zeroize::Zeroizing;

use crate::{HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LEN, ML_DSA_PUBLIC_KEY_LEN, address_of};

/// Length of a BIP-39 seed.
pub const SEED_LEN: usize = 64;
/// SLIP-44 coin type, copied from `maya_wallet_core::hd::COIN_TYPE`.
pub const COIN_TYPE: u32 = 2992;
/// The hardened-index bit.
const HARDENED: u32 = 0x8000_0000;
/// SLIP-0010 curve seed: the wallet uses the ed25519 construction for its
/// hardened-only chain, then feeds the 32-byte key to the PQ split below.
const SLIP10_CURVE: &[u8] = b"ed25519 seed";
/// BIP-39 PBKDF2 rounds, fixed by the specification.
const PBKDF2_ROUNDS: u32 = 2048;
/// Copied from `custom_l1_node::crypto::hybrid`.
const LATTICE_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.ml-dsa-65.v1";
/// Copied from `custom_l1_node::crypto::hybrid`.
const HASH_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.slh-dsa-sha2-128s.v1";
/// Copied from `custom_l1_node::core::transaction` (the name says v4; the
/// wire version is 5 — the domain did not change when the frame did).
const TX_DOMAIN: &[u8] = b"custom-l1-node.tx.v4";
/// Plain-transfer wire version.
const WIRE_VERSION: u8 = 5;
/// The all-zero `rnd` selecting FIPS 204 deterministic signing, as the node.
const DETERMINISTIC_SEED: [u8; 32] = [0u8; 32];
/// Bytes of one encoded output: 32-byte LE amount plus a 32-byte address.
const OUTPUT_LEN: usize = 64;

/// Where fees are paid: the node's `state::fees::FEE_COLLECTOR`, copied
/// (`b"maya2c/fee/collector/v1"`, zero-padded). Pinned by the parity tests.
pub const FEE_COLLECTOR: [u8; 32] = *b"maya2c/fee/collector/v1\0\0\0\0\0\0\0\0\0";

/// Pinned mainnet genesis ids, lower-case hex: a copy of
/// `maya_wallet_core::networks::MAINNET_GENESIS` (the parity tests fail when
/// the two differ). The genesis ceremony runbook adds the id to both.
pub const MAINNET_GENESIS: &[&str] = &[];
/// Pinned public testnet genesis ids: a copy of the desktop's `TESTNET_GENESIS`.
pub const TESTNET_GENESIS: &[&str] = &[];

/// Argon2id memory in KiB: the desktop vault's parameter (`vault.rs`).
pub const KDF_MEMORY_KIB: u32 = 64 * 1024;
/// Argon2id passes: the desktop vault's parameter.
pub const KDF_ITERATIONS: u32 = 3;
/// Argon2id lanes: the desktop vault's parameter.
pub const KDF_LANES: u32 = 1;

/// A BIP-39 mnemonic for caller-supplied entropy (16-32 bytes).
///
/// Entropy comes from the caller because in a browser the source is
/// `crypto.getRandomValues`, and keeping the draw outside makes this testable.
///
/// # Errors
///
/// Returns the reason when the entropy length is not one BIP-39 allows.
pub fn mnemonic_from_entropy(entropy: &[u8]) -> Result<Zeroizing<String>, String> {
    bip39::Mnemonic::from_entropy(entropy)
        .map(|m| Zeroizing::new(m.to_string()))
        .map_err(|e| e.to_string())
}

/// Checks a mnemonic's words and checksum.
///
/// # Errors
///
/// Returns the reason the phrase is refused.
pub fn validate_mnemonic(phrase: &str) -> Result<(), String> {
    bip39::Mnemonic::parse_normalized(phrase.trim())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// BIP-39 seed: PBKDF2-HMAC-SHA512 over the trimmed phrase, salt
/// `"mnemonic" || passphrase` — the same bytes `hd::seed_from_mnemonic` uses.
///
/// # Errors
///
/// Returns the reason when the phrase is invalid.
pub fn seed_from_mnemonic(
    phrase: &str,
    passphrase: &str,
) -> Result<Zeroizing<[u8; SEED_LEN]>, String> {
    validate_mnemonic(phrase)?;
    let mut salt = Zeroizing::new(String::with_capacity(8 + passphrase.len()));
    salt.push_str("mnemonic");
    salt.push_str(passphrase);
    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    pbkdf2::pbkdf2::<Hmac<Sha512>>(
        phrase.trim().as_bytes(),
        salt.as_bytes(),
        PBKDF2_ROUNDS,
        seed.as_mut(),
    )
    .map_err(|e| e.to_string())?;
    Ok(seed)
}

/// One SLIP-0010 HMAC step; returns `(key, chain_code)` as one 64-byte block.
fn slip10_step(hmac_key: &[u8], parts: &[&[u8]]) -> Result<Zeroizing<[u8; 64]>, String> {
    let mut mac = Hmac::<Sha512>::new_from_slice(hmac_key).map_err(|e| e.to_string())?;
    for part in parts {
        mac.update(part);
    }
    let mut out = Zeroizing::new([0u8; 64]);
    out.copy_from_slice(&mac.finalize().into_bytes());
    Ok(out)
}

/// The SLIP-0010 chain key at `m/44'/2992'/account'/0'/index'`.
fn chain_key_at(seed: &[u8], account: u32, index: u32) -> Result<Zeroizing<[u8; 32]>, String> {
    if seed.len() != SEED_LEN {
        return Err(format!("seed is {} bytes, not {SEED_LEN}", seed.len()));
    }
    if account >= HARDENED || index >= HARDENED {
        return Err("account and index must be below 2^31".to_string());
    }
    let mut node = slip10_step(SLIP10_CURVE, &[seed])?;
    for step in [44, COIN_TYPE, account, 0, index] {
        let (key, chain_code) = node.split_at(32);
        // The leading zero is the ed25519 construction's private-key marker.
        let next = slip10_step(chain_code, &[&[0u8], key, &(step | HARDENED).to_be_bytes()])?;
        node = next;
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&node[..32]);
    Ok(key)
}

/// `blake3(domain || chain_key)`, the node's `derive_seed`.
fn split_seed(domain: &[u8], chain_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(chain_key);
    Zeroizing::new(*hasher.finalize().as_bytes())
}

/// Feeds a fixed 32-byte seed to `fips204` key generation, as the node's
/// `keys::SeedRng` does.
struct SeedRng(Zeroizing<[u8; 32]>);

impl fips204::RngCore for SeedRng {
    fn next_u32(&mut self) -> u32 {
        // reviewed-guard: fips204 keygen draws only fill_bytes, as in the node.
        unimplemented!("ML-DSA key generation draws only the 32-byte seed")
    }
    fn next_u64(&mut self) -> u64 {
        // reviewed-guard: fips204 keygen draws only fill_bytes, as in the node.
        unimplemented!("ML-DSA key generation draws only the 32-byte seed")
    }
    fn fill_bytes(&mut self, out: &mut [u8]) {
        // A partial fill would be a key the seed does not name; panic instead.
        assert_eq!(out.len(), 32, "ML-DSA keygen asked for {} bytes", out.len());
        out.copy_from_slice(self.0.as_ref());
    }
    fn try_fill_bytes(&mut self, out: &mut [u8]) -> Result<(), fips204::RngError> {
        self.fill_bytes(out);
        Ok(())
    }
}

impl fips204::CryptoRng for SeedRng {}

/// One account's two signing keys.
struct AccountKey {
    lattice: ml_dsa_65::PrivateKey,
    hash_based: slh::SigningKey,
}

impl AccountKey {
    fn derive(seed: &[u8], account: u32, index: u32) -> Result<Self, String> {
        let chain_key = chain_key_at(seed, account, index)?;
        let mut rng = SeedRng(split_seed(LATTICE_SEED_DOMAIN, &chain_key));
        let (_, lattice) = ml_dsa_65::KG::try_keygen_with_rng(&mut rng)
            .map_err(|e| format!("ML-DSA key generation failed: {e}"))?;
        let hash_based = slh::signing_key_from_seed(&split_seed(HASH_SEED_DOMAIN, &chain_key));
        Ok(Self {
            lattice,
            hash_based,
        })
    }

    fn public_key(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HYBRID_PUBLIC_KEY_LEN);
        out.extend_from_slice(&self.lattice.get_public_key().into_bytes());
        out.extend_from_slice(&self.hash_based.verifying_key().to_bytes());
        out
    }
}

/// The hybrid public key (1984 bytes) of account `index`.
///
/// # Errors
///
/// Returns the reason when the seed or indices are invalid.
pub fn public_key_at(seed: &[u8], account: u32, index: u32) -> Result<Vec<u8>, String> {
    Ok(AccountKey::derive(seed, account, index)?.public_key())
}

/// The hex address of account `index`: `address_of(public_key_at(..))`.
///
/// # Errors
///
/// Returns the reason when the seed or indices are invalid.
pub fn address_at(seed: &[u8], account: u32, index: u32) -> Result<String, String> {
    address_of(&public_key_at(seed, account, index)?)
}

/// A plain value transfer: one payment output and, when `fee` is not zero, a
/// fee output — the shape `maya_wallet_core::payment::transfer_tx` builds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    /// Recipient address.
    pub recipient: [u8; 32],
    /// Amount in base units, 256-bit little-endian (the node's `put_amount`).
    pub amount: [u8; 32],
    /// Fee in base units, 256-bit little-endian. Zero adds no output.
    pub fee: [u8; 32],
    /// Where the fee goes.
    pub fee_recipient: [u8; 32],
    /// The sender's next nonce.
    pub nonce: u64,
}

impl Transfer {
    fn has_fee(&self) -> bool {
        self.fee.iter().any(|b| *b != 0)
    }

    /// Inputs, outputs, public key and nonce: the node's `body_bytes` for a
    /// transfer (whose kind appends nothing).
    fn body(&self, public_key: &[u8]) -> Vec<u8> {
        let outputs: u64 = if self.has_fee() { 2 } else { 1 };
        let mut buf = Vec::with_capacity(16 + 2 * OUTPUT_LEN + HYBRID_PUBLIC_KEY_LEN + 8);
        buf.extend_from_slice(&0u64.to_le_bytes());
        buf.extend_from_slice(&outputs.to_le_bytes());
        buf.extend_from_slice(&self.amount);
        buf.extend_from_slice(&self.recipient);
        if self.has_fee() {
            buf.extend_from_slice(&self.fee);
            buf.extend_from_slice(&self.fee_recipient);
        }
        buf.extend_from_slice(public_key);
        buf.extend_from_slice(&self.nonce.to_le_bytes());
        buf
    }

    /// What both signatures cover: `TX_DOMAIN || chain || body`.
    #[must_use]
    pub fn signing_bytes(&self, public_key: &[u8], chain: &[u8; 32]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(TX_DOMAIN.len() + 32 + 2 * OUTPUT_LEN + 2100);
        buf.extend_from_slice(TX_DOMAIN);
        buf.extend_from_slice(chain);
        buf.extend_from_slice(&self.body(public_key));
        buf
    }
}

/// Encoded length of a signed transfer, for the per-byte fee quote.
#[must_use]
pub const fn transfer_wire_len(with_fee: bool) -> usize {
    let outputs = if with_fee { 2 } else { 1 };
    1 + 8 + 8 + outputs * OUTPUT_LEN + HYBRID_PUBLIC_KEY_LEN + 8 + 1 + HYBRID_SIGNATURE_LEN
}

/// The address prefix a confirm screen writes addresses in on the chain
/// `chain`: decided by the pinned genesis lists alone, as the desktop's
/// `networks::address_network` decides it. An unpinned chain is `dmaya`.
#[must_use]
pub fn address_network(chain: &[u8; 32]) -> Network {
    let id = hex::encode(chain);
    if MAINNET_GENESIS.iter().any(|g| g.eq_ignore_ascii_case(&id)) {
        Network::Mainnet
    } else if TESTNET_GENESIS.iter().any(|g| g.eq_ignore_ascii_case(&id)) {
        Network::Testnet
    } else {
        Network::Devnet
    }
}

/// What the confirm screen shows for a transfer, computed from the bytes that
/// will be signed — the fields of the desktop's `confirm::Confirmation` that a
/// plain transfer has, through the same `maya_clear_sign` renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferConfirmation {
    /// Lowercase hex BLAKE3 of the signing bytes. [`sign_transfer`] signs
    /// only when handed this back.
    pub signing_hash: String,
    /// `maya_clear_sign::render` of the payment, recipient in full Bech32m.
    pub description: String,
    /// Recipient, grouped Bech32m.
    pub recipient: String,
    /// Sender, grouped Bech32m.
    pub sender: String,
    /// The address prefix, e.g. `dmaya`.
    pub address_prefix: String,
    /// `maya_clear_sign::review` of the payment, most severe first, in the
    /// desktop's words.
    pub warnings: Vec<String>,
}

/// A fee may go only to the fee collector. Anywhere else it is a second
/// payment the confirm screen would not show as one.
fn check_fee_recipient(transfer: &Transfer) -> Result<(), String> {
    if transfer.has_fee() && transfer.fee_recipient != FEE_COLLECTOR {
        return Err("a fee may only be paid to the fee collector; nothing was signed".to_string());
    }
    Ok(())
}

fn amount_of(le: &[u8; 32]) -> Amount {
    let mut limbs = [0u64; 4];
    for (limb, chunk) in limbs.iter_mut().zip(le.chunks_exact(8)) {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(chunk);
        *limb = u64::from_le_bytes(bytes);
    }
    Amount(limbs)
}

/// The confirmation for `transfer` from account `index` on `chain`. Signs
/// nothing. `balance` (32-byte little-endian), when known, lets the review
/// warn about a transfer that drains the account.
///
/// # Errors
///
/// Returns the reason when derivation fails or the fee goes elsewhere than
/// the fee collector.
pub fn confirm_transfer(
    seed: &[u8],
    account: u32,
    index: u32,
    transfer: &Transfer,
    chain: &[u8; 32],
    balance: Option<&[u8; 32]>,
) -> Result<TransferConfirmation, String> {
    check_fee_recipient(transfer)?;
    let public_key = public_key_at(seed, account, index)?;
    let mut sender = [0u8; 32];
    hex::decode_to_slice(address_of(&public_key)?, &mut sender).map_err(|e| e.to_string())?;
    let network = address_network(chain);
    // The fee output pays the collector, so the one effect is the payment, as
    // the desktop's `effects_of` reads the same transaction.
    let effects = [Effect::Transfer {
        token: [0; 32],
        to: transfer.recipient,
        amount: amount_of(&transfer.amount),
    }];
    let ctx = Context {
        balances: balance
            .map(|b| ([0; 32], amount_of(b)))
            .into_iter()
            .collect(),
        ..Context::default()
    };
    Ok(TransferConfirmation {
        signing_hash: signing_hash(&transfer.signing_bytes(&public_key, chain)),
        description: render(&effects[0], network),
        recipient: grouped(network, &transfer.recipient),
        sender: grouped(network, &sender),
        address_prefix: network.hrp().to_string(),
        // The wallet built the transaction, so the claim is its own simulation.
        warnings: review(&effects, &effects, &ctx)
            .into_iter()
            .map(|w| warning_text(w).to_string())
            .collect(),
    })
}

fn signing_hash(signing_bytes: &[u8]) -> String {
    hex::encode(blake3::hash(signing_bytes).as_bytes())
}

/// Signs `transfer` for the chain whose genesis id is `chain`, returning the
/// wire bytes the gateway's `/v1/transactions` takes (hex-encoded) — but only
/// the transfer `confirmed_hash` names: the [`TransferConfirmation::signing_hash`]
/// the confirm screen showed. Any difference (recipient, amount, fee, nonce,
/// chain, account) changes the hash and nothing is signed.
///
/// # Errors
///
/// Returns the reason when the hash does not match, the fee goes elsewhere
/// than the fee collector, or derivation or ML-DSA signing fails.
pub fn sign_transfer(
    seed: &[u8],
    account: u32,
    index: u32,
    transfer: &Transfer,
    chain: &[u8; 32],
    confirmed_hash: &str,
) -> Result<Vec<u8>, String> {
    check_fee_recipient(transfer)?;
    let key = AccountKey::derive(seed, account, index)?;
    let public_key = key.public_key();
    let message = transfer.signing_bytes(&public_key, chain);
    if !signing_hash(&message).eq_ignore_ascii_case(confirmed_hash.trim()) {
        return Err(
            "the transaction differs from the one on the confirm screen; nothing was signed"
                .to_string(),
        );
    }
    let lattice_sig = key
        .lattice
        .try_sign_with_seed(&DETERMINISTIC_SEED, &message, b"")
        .map_err(|e| format!("ML-DSA signing failed: {e}"))?;
    let hash_sig = key.hash_based.sign(&message);

    let body = transfer.body(&public_key);
    let mut wire = Vec::with_capacity(transfer_wire_len(transfer.has_fee()));
    wire.push(WIRE_VERSION);
    // The wire frame is the body with the signature flag and pair spliced in
    // after the nonce; a transfer's kind adds nothing after it.
    wire.extend_from_slice(&body);
    wire.push(1);
    wire.extend_from_slice(&lattice_sig);
    wire.extend_from_slice(&hash_sig);
    debug_assert_eq!(public_key.len(), HYBRID_PUBLIC_KEY_LEN);
    debug_assert_eq!(lattice_sig.len(), crate::ML_DSA_SIGNATURE_LEN);
    Ok(wire)
}

const _: () = assert!(ML_DSA_PUBLIC_KEY_LEN + 32 == HYBRID_PUBLIC_KEY_LEN);

/// The vault key: Argon2id with the desktop vault's parameters, 32 bytes.
///
/// # Errors
///
/// Returns the reason when the salt is shorter than Argon2 allows.
pub fn vault_key(password: &[u8], salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let params = argon2::Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_LANES, Some(32))
        .map_err(|e| e.to_string())?;
    let kdf = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    kdf.hash_password_into(password, salt, key.as_mut())
        .map_err(|e| e.to_string())?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                          abandon abandon abandon about";

    #[test]
    fn the_bip39_seed_matches_the_published_trezor_vector() {
        // BIP-39 reference vector (passphrase "TREZOR"), first eight bytes.
        let seed = seed_from_mnemonic(PHRASE, "TREZOR").unwrap();
        assert_eq!(hex::encode(&seed[..8]), "c55257c360c07c72");
    }

    #[test]
    fn a_bad_checksum_is_refused() {
        let bad = PHRASE.replace("about", "abandon");
        assert!(seed_from_mnemonic(&bad, "").is_err());
    }

    #[test]
    fn signing_is_deterministic_and_verifies() {
        let seed = seed_from_mnemonic(PHRASE, "").unwrap();
        let t = Transfer {
            recipient: [7; 32],
            amount: [1; 32],
            fee: [0; 32],
            fee_recipient: [0; 32],
            nonce: 3,
        };
        let hash = confirm_transfer(seed.as_ref(), 0, 0, &t, &[9; 32], None)
            .unwrap()
            .signing_hash;
        let a = sign_transfer(seed.as_ref(), 0, 0, &t, &[9; 32], &hash).unwrap();
        let b = sign_transfer(seed.as_ref(), 0, 0, &t, &[9; 32], &hash).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.len(), transfer_wire_len(false));
        let pk = public_key_at(seed.as_ref(), 0, 0).unwrap();
        let sig = &a[a.len() - HYBRID_SIGNATURE_LEN..];
        assert!(crate::verify_hybrid(
            &pk,
            &t.signing_bytes(&pk, &[9; 32]),
            sig
        ));
        // Another chain's tag does not verify: the ADR-036 binding.
        assert!(!crate::verify_hybrid(
            &pk,
            &t.signing_bytes(&pk, &[8; 32]),
            sig
        ));
    }

    fn transfer(nonce: u64) -> Transfer {
        Transfer {
            recipient: [7; 32],
            amount: [1; 32],
            fee: [0; 32],
            fee_recipient: [0; 32],
            nonce,
        }
    }

    #[test]
    fn a_hash_for_another_transfer_or_chain_signs_nothing() {
        let seed = seed_from_mnemonic(PHRASE, "").unwrap();
        let shown = confirm_transfer(seed.as_ref(), 0, 0, &transfer(3), &[9; 32], None).unwrap();
        // Same hash, one field changed: nonce, chain, account.
        let err = sign_transfer(
            seed.as_ref(),
            0,
            0,
            &transfer(4),
            &[9; 32],
            &shown.signing_hash,
        )
        .unwrap_err();
        assert!(err.contains("nothing was signed"), "{err}");
        assert!(
            sign_transfer(
                seed.as_ref(),
                0,
                0,
                &transfer(3),
                &[8; 32],
                &shown.signing_hash
            )
            .is_err()
        );
        assert!(
            sign_transfer(
                seed.as_ref(),
                1,
                0,
                &transfer(3),
                &[9; 32],
                &shown.signing_hash
            )
            .is_err()
        );
        assert!(sign_transfer(seed.as_ref(), 0, 0, &transfer(3), &[9; 32], "").is_err());
    }

    #[test]
    fn a_fee_paid_anywhere_but_the_collector_is_refused() {
        let seed = seed_from_mnemonic(PHRASE, "").unwrap();
        let mut t = transfer(3);
        t.fee = [2; 32];
        t.fee_recipient = [7; 32];
        assert!(confirm_transfer(seed.as_ref(), 0, 0, &t, &[9; 32], None).is_err());
        t.fee_recipient = FEE_COLLECTOR;
        assert!(confirm_transfer(seed.as_ref(), 0, 0, &t, &[9; 32], None).is_ok());
    }

    #[test]
    fn the_confirmation_names_the_unpinned_chain_dmaya_and_warns_on_a_drain() {
        let seed = seed_from_mnemonic(PHRASE, "").unwrap();
        let mut t = transfer(3);
        t.amount = [0; 32];
        t.amount[0] = 95;
        let mut balance = [0u8; 32];
        balance[0] = 100;
        let shown = confirm_transfer(seed.as_ref(), 0, 0, &t, &[9; 32], Some(&balance)).unwrap();
        assert_eq!(shown.address_prefix, "dmaya");
        assert!(
            shown.recipient.starts_with("dmaya1 "),
            "{}",
            shown.recipient
        );
        assert!(
            shown.description.contains("95 base units"),
            "{}",
            shown.description
        );
        assert_eq!(
            shown.warnings,
            vec![warning_text(maya_clear_sign::Warning::DrainsBalance).to_string()]
        );
    }

    #[test]
    fn indices_at_or_above_two_to_the_31_are_refused() {
        let seed = [0u8; SEED_LEN];
        assert!(public_key_at(&seed, HARDENED, 0).is_err());
        assert!(public_key_at(&seed[..10], 0, 0).is_err());
    }
}
