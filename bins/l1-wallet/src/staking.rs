//! Staking transactions (ADR-028) from the wallet: how an independent operator
//! puts a validator key into the committee, and how anyone delegates to one.
//!
//! Two keys meet here and stay distinct. The wallet's hybrid key pays the bond
//! and signs the transaction; the validator's ML-DSA-65 key — the one the node
//! votes with, written by `maya2c-node --generate-validator-key` — signs only a
//! proof of possession over the paying address. Without that proof anyone
//! could register someone else's key and capture its votes.

use std::path::Path;

use anyhow::{Context, Result, bail};
use custom_l1_node::core::staking_payload::StakingAction;
use custom_l1_node::crypto::keys::{SECRET_KEY_LEN, SigningKey};
use zeroize::Zeroizing;

/// Commission is in basis points; above 100 % would pay the operator more than
/// the rewards the validator earned.
pub const MAX_COMMISSION_BPS: u16 = 10_000;

/// Reads a validator key in the format `maya2c-node --generate-validator-key`
/// writes: the ML-DSA-65 secret key as hex.
///
/// # Errors
///
/// The file is unreadable, not hex, or not a secret key's length.
pub fn load_validator_key(path: &Path) -> Result<SigningKey> {
    let text = Zeroizing::new(
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    );
    let bytes = Zeroizing::new(
        hex::decode(text.trim()).with_context(|| format!("{} is not hex", path.display()))?,
    );
    let Ok(array) = <[u8; SECRET_KEY_LEN]>::try_from(bytes.as_slice()) else {
        bail!("{}: not an ML-DSA-65 validator secret key", path.display());
    };
    let array = Zeroizing::new(array);
    SigningKey::from_bytes(&array).context("decoding the validator key")
}

/// The registration `sender` submits for `validator`, bonding `bond`.
///
/// # Errors
///
/// A commission above 100 %, a zero bond, or a failed possession signature.
pub fn register(
    validator: &SigningKey,
    sender: &[u8; 32],
    bond: u64,
    commission_bps: u16,
) -> Result<StakingAction> {
    if commission_bps > MAX_COMMISSION_BPS {
        bail!("commission {commission_bps} bps is above 100 % ({MAX_COMMISSION_BPS} bps)");
    }
    if bond == 0 {
        bail!("a registration must bond something");
    }
    let possession = validator
        .sign(&StakingAction::possession_message(sender))
        .context("signing the proof of possession")?;
    Ok(StakingAction::Register {
        key: Box::new(validator.verifying_key().to_bytes()),
        bond,
        commission_bps,
        possession: Box::new(possession),
    })
}

/// A validator's id, as `delegate` takes it: the hash of its public key.
#[must_use]
pub fn validator_id(validator: &SigningKey) -> [u8; 32] {
    custom_l1_node::state::staking::validator_id(&validator.verifying_key().to_bytes())
}

/// Parses a 64-character hex validator id.
///
/// # Errors
///
/// Not hex, or not 32 bytes.
pub fn parse_validator_id(text: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(text.trim()).context("validator id is not hex")?;
    <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| anyhow::anyhow!("validator id must be 32 bytes (64 hex characters)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use custom_l1_node::crypto::keys::generate_signing_key;

    #[test]
    fn a_registration_proves_possession_of_the_key_for_this_sender_only() {
        let key = generate_signing_key().unwrap();
        let sender = [7u8; 32];

        let StakingAction::Register {
            key: public,
            possession,
            bond,
            ..
        } = register(&key, &sender, 5_000, 500).unwrap()
        else {
            panic!("not a registration");
        };

        assert_eq!(bond, 5_000);
        let vk = key.verifying_key();
        assert_eq!(*public, vk.to_bytes());
        assert!(
            vk.verify(&StakingAction::possession_message(&sender), &possession)
                .is_ok()
        );
        // Replayed by another sender, the proof says nothing.
        assert!(
            vk.verify(&StakingAction::possession_message(&[8u8; 32]), &possession)
                .is_err()
        );
    }

    #[test]
    fn an_impossible_commission_or_empty_bond_is_refused() {
        let key = generate_signing_key().unwrap();
        assert!(register(&key, &[0; 32], 1, MAX_COMMISSION_BPS + 1).is_err());
        assert!(register(&key, &[0; 32], 0, 0).is_err());
    }

    #[test]
    fn the_node_key_file_format_round_trips() {
        let key = generate_signing_key().unwrap();
        let dir = std::env::temp_dir().join(format!("l1w-vk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("validator.key");
        std::fs::write(&path, hex::encode(key.to_bytes().as_slice())).unwrap();

        let loaded = load_validator_key(&path).unwrap();
        assert_eq!(
            loaded.verifying_key().to_bytes(),
            key.verifying_key().to_bytes()
        );

        std::fs::write(&path, "abcd").unwrap();
        assert!(load_validator_key(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_validator_id_parses_only_at_full_length() {
        assert_eq!(parse_validator_id(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert!(parse_validator_id("abcd").is_err());
        assert!(parse_validator_id(&"zz".repeat(32)).is_err());
    }
}
