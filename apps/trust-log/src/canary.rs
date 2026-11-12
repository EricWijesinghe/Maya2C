//! `foundation-canary`: a signed statement that names a recent block.
//!
//! The block id cannot be known before the block exists, so a canary cannot
//! be signed in advance and left to publish itself. [`check`] fails when the
//! named block is not the chain's block at that height, or when it is older
//! than `max_age_blocks` — a canary that stopped being renewed has lapsed,
//! and saying so is the whole point.
//!
//! The statement's wording is the operator's (legal review pending); this
//! module supplies no default text.

use serde::{Deserialize, Serialize};

use crate::anchor::Anchor;
use crate::chain::{self, Chain};
use crate::key::SignerKey;
use crate::signed::{self, Signed};
use crate::{Error, Result};

/// Digest domain of a canary.
pub const DOMAIN: &str = "maya2c 2026-11-12 foundation-canary v1";
/// The schema each canary is anchored under.
pub const SCHEMA: &str = "maya2c/canary/v1";

/// One canary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canary {
    /// The statement, verbatim.
    pub statement: String,
    /// Height of the block it names.
    pub block_height: u64,
    /// Hex id of that block.
    pub block_id: String,
}

/// Signs `statement` over the chain's current tip.
///
/// # Errors
///
/// [`Error::Malformed`] for an empty statement; as [`Chain`] and
/// [`signed::seal`].
pub fn sign(
    key: &SignerKey,
    chain: &dyn Chain,
    statement: &str,
) -> Result<(Signed<Canary>, Anchor)> {
    if statement.trim().is_empty() {
        return Err(Error::Malformed("canary statement is empty".into()));
    }
    let block_height = chain.tip_height()?;
    let body = Canary {
        statement: statement.to_owned(),
        block_height,
        block_id: chain::block_id(chain, block_height)?,
    };
    let canary = signed::seal(key, DOMAIN, body)?;
    let anchor = Anchor::new(
        SCHEMA.to_owned(),
        crate::hex_array("digest", &canary.digest)?,
    )?;
    Ok((canary, anchor))
}

/// Checks a canary against the pinned key and the chain, and returns its
/// anchor.
///
/// # Errors
///
/// [`Error::Signature`] for a canary the pinned key did not sign, and
/// [`Error::Lapsed`] for one naming a block the chain does not have at that
/// height, or one older than `max_age_blocks`.
pub fn check(
    canary: &Signed<Canary>,
    pinned: &[u8],
    chain: &dyn Chain,
    max_age_blocks: u64,
) -> Result<Anchor> {
    let digest = canary.open(DOMAIN, pinned)?;
    let body = &canary.body;
    let on_chain = chain::block_id(chain, body.block_height)
        .map_err(|e| Error::Lapsed(format!("block {} cannot be read: {e}", body.block_height)))?;
    if on_chain != body.block_id {
        return Err(Error::Lapsed(format!(
            "names block id {} at height {}, the chain has {on_chain}",
            body.block_id, body.block_height
        )));
    }
    let age = chain.tip_height()?.saturating_sub(body.block_height);
    if age > max_age_blocks {
        return Err(Error::Lapsed(format!(
            "{age} blocks old, limit {max_age_blocks}"
        )));
    }
    Anchor::new(SCHEMA.to_owned(), digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::tests::{SENDER, anchored};
    use crate::chain::fake::FakeChain;
    use crate::signed::tests::key;

    const TEXT: &str = "No warrant has been served on the foundation.";

    fn chain_at(tip: u64) -> FakeChain {
        let mut chain = FakeChain::default();
        let a = Anchor::new("filler".into(), [0; 32]).unwrap();
        for h in 0..=tip {
            anchored(&mut chain, h, &a, &SENDER);
        }
        chain
    }

    #[test]
    fn a_fresh_canary_passes() {
        let (k, chain) = (key(1), chain_at(20));
        let (c, a) = sign(&k, &chain, TEXT).unwrap();
        assert_eq!(c.body.block_height, 20);
        assert_eq!(check(&c, k.public_key(), &chain, 10).unwrap(), a);
    }

    #[test]
    fn an_unrenewed_canary_lapses() {
        let k = key(1);
        let (c, _) = sign(&k, &chain_at(20), TEXT).unwrap();
        let later = chain_at(31);
        assert!(matches!(
            check(&c, k.public_key(), &later, 10),
            Err(Error::Lapsed(_))
        ));
        assert!(check(&c, k.public_key(), &later, 11).is_ok());
    }

    #[test]
    fn a_pre_signed_canary_with_a_guessed_block_id_lapses() {
        let k = key(1);
        let body = Canary {
            statement: TEXT.into(),
            block_height: 20,
            block_id: "00".repeat(32),
        };
        let c = signed::seal(&k, DOMAIN, body).unwrap();
        assert!(matches!(
            check(&c, k.public_key(), &chain_at(20), 10),
            Err(Error::Lapsed(_))
        ));
    }

    #[test]
    fn a_canary_from_another_key_is_refused() {
        let chain = chain_at(5);
        let (c, _) = sign(&key(9), &chain, TEXT).unwrap();
        assert!(matches!(
            check(&c, key(1).public_key(), &chain, 10),
            Err(Error::Signature(_))
        ));
    }

    #[test]
    fn an_empty_statement_is_refused() {
        assert!(sign(&key(1), &chain_at(1), "  ").is_err());
    }
}
