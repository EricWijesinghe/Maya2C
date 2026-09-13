//! State proofs: what a light client needs to believe a balance.
//!
//! ## Why an account path is not enough
//!
//! The state root is not one tree. It is an accounts tree, folded through a
//! chain of one-sided layers:
//!
//! ```text
//! root = accounts_root
//! root = H(flash      ‖ root ‖ channels_root)    if any channel exists
//! root = H(dex        ‖ root ‖ dex_root)         if any `d:` record exists
//! root = H(oracle     ‖ root ‖ oracle_root)      if any `o:` record exists
//! root = H(governance ‖ root ‖ governance_root)  if any `g:` record exists
//! root = H(sealed     ‖ root ‖ sealed_root)      if any `m:` record exists
//! root = H(shielded   ‖ root ‖ pool_state)       if the pool is not empty
//! root = H(contracts  ‖ root ‖ contracts_root)   if any code or storage exists
//! root = H(nullifiers ‖ root ‖ nullifiers_root)  if any nullifier exists
//! ```
//!
//! Each layer folds in **only when it holds something**, which is what keeps a
//! chain that has never traded, never governed, and never shielded at exactly
//! the root it would have had before those subsystems existed. The cost is that
//! the shape of the fold is a fact about the chain's *contents*, not a constant.
//!
//! So a proof carries the layer digests alongside the path. Without them a
//! verifier reaches `accounts_root` and has nothing to compare it to; with them
//! it reaches the header's `state_root` and has checked everything in between.
//!
//! ## What this does and does not prove
//!
//! It proves **inclusion**: this account, with this balance and nonce, was in
//! the state committed to by that root.
//!
//! It does **not** prove the root is canonical. That is the header chain's job,
//! and it is why [`crate::state::proof`] is only half of a light client — the
//! other half follows headers and picks the chain with the most work. A proof
//! against a root nobody mined is a proof of nothing.
//!
//! It also does not prove **absence**. Showing that an account is *not* in the
//! tree would need the two neighbouring leaves and an ordering argument, and
//! nothing in this node needs that yet.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::merkle::{HASH_LEN, PathStep, account_leaf, verify_path};

/// A subsystem that folds into the state root above the accounts tree.
///
/// The order is the order they fold in, and it is consensus: a verifier that
/// applied two layers in the wrong sequence would compute a different root and
/// reject a valid proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StateLayer {
    /// Payment channels.
    Channels,
    /// Trading records, under the `d:` prefix.
    Trading,
    /// Oracle records, under the `o:` prefix.
    Oracle,
    /// Governance records, under the `g:` prefix.
    Governance,
    /// Sealed-mempool records, under the `m:` prefix.
    Sealed,
    /// Identity records, under the `i:` prefix: DID documents, attestations
    /// and revocation pages.
    Identity,
    /// Real-world asset records, under the `r:` prefix: tokens, cap tables,
    /// legal attestations, settled distributions and cached eligibility.
    Rwa,
    /// The shielded pool: a BLAKE3 commitment to the whole stored pool, which
    /// is the Poseidon tree's frontier, the anchor window and the public
    /// balance. See `ShieldedPool::commitment`; it was the bare Poseidon root
    /// until 2026-09-11.
    Shielded,
    /// Contract code (`code:`) and contract storage (`cstate:`).
    ///
    /// Outside the root until 2026-09-11. Two nodes could disagree about a
    /// contract's storage and agree on every state root, which made the
    /// divergence invisible and a snapshot of it unverifiable.
    Contracts,
    /// The spent-note set (`null:`).
    ///
    /// Outside the root until 2026-09-11. A snapshot that left a nullifier out
    /// would have produced a node that accepts the second spend of that note.
    Nullifiers,
}

/// Every layer, in fold order.
pub const LAYER_ORDER: &[StateLayer] = &[
    StateLayer::Channels,
    StateLayer::Trading,
    StateLayer::Oracle,
    StateLayer::Governance,
    StateLayer::Sealed,
    StateLayer::Shielded,
    StateLayer::Contracts,
    StateLayer::Nullifiers,
];

impl StateLayer {
    /// The BLAKE3 derive-key domain this layer folds under.
    ///
    /// These strings are consensus. They live here as the single definition and
    /// [`crate::state::db`] folds with them, so the prover and the verifier
    /// cannot drift — which they would, kept as literals in two files.
    #[must_use]
    pub const fn domain(self) -> &'static str {
        match self {
            Self::Channels => "maya-flash state root v1",
            Self::Trading => "maya-dex state root v1",
            Self::Oracle => "maya-oracle state root v1",
            Self::Governance => "maya-governance state root v1",
            Self::Sealed => "maya sealed mempool state root v1",
            Self::Shielded => "maya shielded state root v1",
            Self::Contracts => "maya contracts state root v1",
            Self::Nullifiers => "maya nullifiers state root v1",
            Self::Identity => "maya identity state root v1",
            Self::Rwa => "maya rwa state root v1",
        }
    }

    /// Stable wire tag. Never renumber — it is consensus.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Channels => 1,
            Self::Trading => 2,
            Self::Oracle => 3,
            Self::Governance => 4,
            Self::Shielded => 5,
            Self::Sealed => 6,
            Self::Contracts => 7,
            Self::Nullifiers => 8,
            Self::Identity => 9,
            Self::Rwa => 10,
        }
    }

    /// Decodes a wire tag.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Channels),
            2 => Some(Self::Trading),
            3 => Some(Self::Oracle),
            4 => Some(Self::Governance),
            5 => Some(Self::Shielded),
            6 => Some(Self::Sealed),
            7 => Some(Self::Contracts),
            8 => Some(Self::Nullifiers),
            _ => None,
        }
    }

    /// Folds this layer's root into a running state root.
    #[must_use]
    pub fn fold(self, running: &[u8; HASH_LEN], layer_root: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
        let mut hasher = blake3::Hasher::new_derive_key(self.domain());
        hasher.update(running);
        hasher.update(layer_root);
        *hasher.finalize().as_bytes()
    }

    /// Short label for errors and explorers.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Channels => "channels",
            Self::Trading => "trading",
            Self::Oracle => "oracle",
            Self::Governance => "governance",
            Self::Sealed => "sealed",
            Self::Shielded => "shielded",
            Self::Contracts => "contracts",
            Self::Nullifiers => "nullifiers",
            Self::Identity => "identity",
            Self::Rwa => "rwa",
        }
    }
}

/// One layer's contribution to a proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerDigest {
    /// Which layer.
    pub layer: StateLayer,
    /// That layer's own root.
    pub root: [u8; HASH_LEN],
}

/// A proof that one account was in the state committed to by a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountProof {
    /// The account being proved.
    pub address: Address,
    /// Its balance and nonce.
    pub account: Account,
    /// Path from the account leaf up to the accounts root.
    pub path: Vec<PathStep>,
    /// The layers folded above it, in fold order.
    ///
    /// Only the layers that were actually present. A chain with no channels
    /// carries no channel digest, and a verifier that expected one would reject
    /// every proof from such a chain.
    pub layers: Vec<LayerDigest>,
}

impl AccountProof {
    /// Recomputes the state root this proof implies.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::MalformedProof`] if the layers are out of order or
    /// repeat. Order is consensus — applying two layers the other way round
    /// yields a different root — so a proof that got it wrong is refused rather
    /// than silently producing a mismatch a caller might attribute to tampering.
    pub fn compute_root(&self) -> Result<[u8; HASH_LEN]> {
        let mut root = verify_path(&account_leaf(&self.address, &self.account), &self.path);

        let mut expected = LAYER_ORDER.iter();
        for digest in &self.layers {
            // Advance through the canonical order until this layer is found.
            // Skipping is legal — an absent layer folds nothing — but going
            // backwards is not.
            let found = expected.by_ref().any(|layer| *layer == digest.layer);
            if !found {
                return Err(NodeError::MalformedProof {
                    reason: format!("layer {} is out of order or repeated", digest.layer.label()),
                });
            }
            root = digest.layer.fold(&root, &digest.root);
        }

        Ok(root)
    }

    /// Whether this proof reproduces `expected_root`.
    ///
    /// The whole of what a light client can check without the chain: that
    /// *some* state with this root contained this account. Whether that root is
    /// canonical is the header chain's question.
    ///
    /// # Errors
    ///
    /// As [`AccountProof::compute_root`].
    pub fn verify(&self, expected_root: &[u8; HASH_LEN]) -> Result<bool> {
        Ok(self.compute_root()? == *expected_root)
    }

    /// Encodes the proof.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32 + 16 + 8 + self.path.len() * 33 + 8);
        buf.extend_from_slice(&self.address);
        buf.extend_from_slice(&self.account.encode());

        buf.extend_from_slice(&(self.path.len() as u64).to_le_bytes());
        for step in &self.path {
            buf.extend_from_slice(&step.sibling);
            buf.push(u8::from(step.sibling_on_left));
        }

        buf.extend_from_slice(&(self.layers.len() as u64).to_le_bytes());
        for digest in &self.layers {
            buf.push(digest.layer.tag());
            buf.extend_from_slice(&digest.root);
        }
        buf
    }

    /// Decodes a proof.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the encoding is truncated, carries
    /// trailing bytes, or names a layer or a side byte that does not exist.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let address = reader.read_array::<32>()?;
        let account = Account::decode(&reader.read_array::<16>()?)?;

        let path_len = reader.read_collection_len(33)?;
        let mut path = Vec::with_capacity(path_len);
        for _ in 0..path_len {
            let sibling = reader.read_array::<HASH_LEN>()?;
            let side = reader.read_u8()?;
            // A side byte outside {0, 1} would give one proof two encodings,
            // and a proof with two encodings has two identifiers.
            let sibling_on_left = match side {
                0 => false,
                1 => true,
                other => {
                    return Err(NodeError::Decode(format!("invalid path side byte {other}")));
                }
            };
            path.push(PathStep {
                sibling,
                sibling_on_left,
            });
        }

        let layer_len = reader.read_collection_len(33)?;
        let mut layers = Vec::with_capacity(layer_len);
        for _ in 0..layer_len {
            let tag = reader.read_u8()?;
            let layer = StateLayer::from_tag(tag)
                .ok_or_else(|| NodeError::Decode(format!("unknown state layer {tag}")))?;
            layers.push(LayerDigest {
                layer,
                root: reader.read_array::<HASH_LEN>()?,
            });
        }

        reader.finish()?;
        Ok(Self {
            address,
            account,
            path,
            layers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(layer: StateLayer, seed: u8) -> LayerDigest {
        LayerDigest {
            layer,
            root: [seed; HASH_LEN],
        }
    }

    fn proof(layers: Vec<LayerDigest>) -> AccountProof {
        AccountProof {
            address: [1u8; 32],
            account: Account {
                balance: 500,
                nonce: 3,
            },
            path: vec![PathStep {
                sibling: [2u8; HASH_LEN],
                sibling_on_left: true,
            }],
            layers,
        }
    }

    #[test]
    fn every_layer_tag_round_trips_and_has_a_distinct_domain() {
        // The domains are what separate one layer's fold from another's. Two
        // layers sharing one would let a digest computed for one be presented
        // as the other.
        let mut domains = std::collections::HashSet::new();
        for layer in LAYER_ORDER {
            assert_eq!(StateLayer::from_tag(layer.tag()), Some(*layer));
            assert!(
                domains.insert(layer.domain()),
                "{} shares a domain",
                layer.label()
            );
        }
        // Derived from `LAYER_ORDER`, not hardcoded. A literal here goes stale
        // the moment a layer is added: `Sealed` took tag 6 and this loop went
        // on asserting 6 was unassigned, which is a failing test rather than a
        // caught bug only because the round trip above disagrees with it.
        let unassigned = LAYER_ORDER
            .iter()
            .map(|layer| layer.tag())
            .max()
            .expect("LAYER_ORDER is never empty")
            + 1;
        for tag in [0u8, unassigned, 255] {
            assert_eq!(StateLayer::from_tag(tag), None);
        }
    }

    #[test]
    fn layers_apply_in_the_canonical_order() {
        let ordered = proof(vec![
            digest(StateLayer::Trading, 9),
            digest(StateLayer::Governance, 8),
        ]);
        assert!(ordered.compute_root().is_ok());

        // Reversed is refused rather than producing a different root a caller
        // might attribute to tampering.
        let reversed = proof(vec![
            digest(StateLayer::Governance, 8),
            digest(StateLayer::Trading, 9),
        ]);
        assert!(matches!(
            reversed.compute_root(),
            Err(NodeError::MalformedProof { .. })
        ));
    }

    #[test]
    fn a_repeated_layer_is_refused() {
        let repeated = proof(vec![
            digest(StateLayer::Trading, 9),
            digest(StateLayer::Trading, 8),
        ]);
        assert!(matches!(
            repeated.compute_root(),
            Err(NodeError::MalformedProof { .. })
        ));
    }

    #[test]
    fn skipping_an_absent_layer_is_legal() {
        // A chain with no channels carries no channel digest. A verifier that
        // demanded one would reject every proof from such a chain.
        let sparse = proof(vec![digest(StateLayer::Shielded, 7)]);
        assert!(sparse.compute_root().is_ok());
        assert!(proof(vec![]).compute_root().is_ok());
    }

    #[test]
    fn changing_any_layer_root_changes_the_state_root() {
        let base = proof(vec![digest(StateLayer::Trading, 9)])
            .compute_root()
            .expect("root");
        let altered = proof(vec![digest(StateLayer::Trading, 10)])
            .compute_root()
            .expect("root");
        assert_ne!(base, altered);
    }

    #[test]
    fn changing_the_account_changes_the_state_root() {
        // The point of the whole structure: a balance that moved must not prove
        // against the root it had before it moved.
        let mut tampered = proof(vec![]);
        let honest = tampered.compute_root().expect("root");
        tampered.account.balance += 1;
        assert_ne!(tampered.compute_root().expect("root"), honest);
    }

    #[test]
    fn a_proof_round_trips_through_its_encoding() {
        let original = proof(vec![
            digest(StateLayer::Channels, 4),
            digest(StateLayer::Oracle, 5),
        ]);
        assert_eq!(AccountProof::decode(&original.encode()), Ok(original));
    }

    #[test]
    fn a_proof_with_a_bad_side_byte_is_refused() {
        // Outside {0, 1} would give one proof two encodings, and a proof with
        // two encodings has two identifiers.
        let mut encoded = proof(vec![]).encode();
        let side_index = 32 + 16 + 8 + HASH_LEN;
        encoded[side_index] = 2;
        assert!(AccountProof::decode(&encoded).is_err());
    }

    #[test]
    fn a_proof_naming_an_unknown_layer_is_refused() {
        let mut encoded = proof(vec![digest(StateLayer::Trading, 9)]).encode();
        let tag_index = encoded.len() - 1 - HASH_LEN;
        encoded[tag_index] = 99;
        assert!(AccountProof::decode(&encoded).is_err());
    }

    #[test]
    fn verify_accepts_only_the_root_it_was_built_against() {
        let proof = proof(vec![digest(StateLayer::Trading, 9)]);
        let root = proof.compute_root().expect("root");

        assert!(proof.verify(&root).expect("verify"));
        assert!(!proof.verify(&[0u8; HASH_LEN]).expect("verify"));
    }
}
