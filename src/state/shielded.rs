//! The shielded pool: commitment tree, anchor window, and proof verification.
//!
//! ## What the pool stores
//!
//! Two things, and neither reveals anything about who owns what. The
//! commitment tree accumulates note commitments — opaque hashes — and never
//! removes one. The nullifier set records which notes have been spent, without
//! saying which commitment each nullifier retires.
//!
//! Because notes are never removed, a Merkle path stays valid as the tree
//! grows only if the verifier accepts the root the path was built against. That
//! is what the anchor window is for: a spend proves membership under some root
//! from the recent past, not necessarily the current one. Without it a wallet
//! would have to build its proof and get it mined inside a single block, and
//! essentially every shielded transaction would fail.
//!
//! ## Cost, and why it is capped
//!
//! Verifying a joinsplit is a pairing check — measured at roughly 2–4 ms,
//! against about 200 µs for an ML-DSA-65 signature. That is a 10–20× asymmetry
//! between what a transaction costs to make and what it costs every node to
//! check, so the number of joinsplits per block is bounded for the same reason
//! [`MAX_BATCH_CLOSURES`](crate::core::payload::MAX_BATCH_CLOSURES) is.
//!
//! The asymmetry narrowed only because signatures got more expensive, not
//! because proofs got cheaper. It was 50× against ed25519's ~50 µs.
//!
//! ## This pool is not post-quantum
//!
//! Worth stating beside the cost, because the two are easy to conflate.
//! Transaction authorization moved to ML-DSA-65, but joinsplits are proved with
//! Groth16 over BLS12-381 — a pairing-based system whose soundness rests on
//! discrete log. An adversary with a quantum computer cannot forge a transfer
//! and *can* forge a shielded proof, which means minting hidden supply that no
//! audit of the transparent chain would reveal. Closing that gap needs a
//! different proof system, and until then the chain's post-quantum security is
//! the weaker of its two halves.

use std::collections::VecDeque;

use maya_zk_privacy::circuit::JoinSplitPublic;
use maya_zk_privacy::field::{fr_from_bytes, fr_to_bytes};
use maya_zk_privacy::params::{ANCHOR_WINDOW, TREE_DEPTH};
use maya_zk_privacy::prove;
use maya_zk_privacy::tree::CommitmentTree;

use crate::core::codec::ByteReader;
use crate::core::payload::ShieldedJoinSplit;
use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::db::{Overlay, POOL_KEY, StateDB, nullifier_key_bytes};

/// Joinsplits allowed in one block.
///
/// A pairing check is ~50× the cost of a signature check, so an uncapped block
/// would let one cheap transaction impose unbounded work on every node.
pub const MAX_SHIELDED_PER_BLOCK: usize = 64;

/// The shielded pool's consensus state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShieldedPool {
    /// Note commitments, append-only.
    tree: CommitmentTree,
    /// Recent roots a spend may prove against, oldest first.
    anchors: VecDeque<[u8; 32]>,
    /// Value currently held inside the pool.
    ///
    /// Individual note values are hidden, but the pool's total is not: it is
    /// exactly the sum of every `public_in` minus every `public_out` ever
    /// applied, and both of those are in the clear. That is what keeps total
    /// supply auditable even though balances are private — without it, an
    /// inflation bug in the circuit would be undetectable from outside.
    balance: u64,
}

impl Default for ShieldedPool {
    fn default() -> Self {
        Self::new()
    }
}

impl ShieldedPool {
    /// An empty pool whose only valid anchor is the empty root.
    #[must_use]
    pub fn new() -> Self {
        let tree = CommitmentTree::new();
        let mut anchors = VecDeque::with_capacity(ANCHOR_WINDOW);
        anchors.push_back(fr_to_bytes(&tree.root()));
        Self {
            tree,
            anchors,
            balance: 0,
        }
    }

    /// Value currently held inside the pool.
    #[must_use]
    pub fn balance(&self) -> u64 {
        self.balance
    }

    /// Records value crossing the pool boundary.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::BalanceOverflow`] if the pool would overflow, and
    /// [`NodeError::ShieldedBalanceUnderflow`] if a withdrawal exceeds what the
    /// pool holds. The second should be unreachable — the circuit already
    /// enforces conservation — which is exactly why it is checked rather than
    /// assumed: it is the tripwire for a soundness break in the proof system.
    pub fn settle(&mut self, public_in: u64, public_out: u64, fee: u64) -> Result<()> {
        // The fee is paid out of shielded value to a transparent sink, so it
        // leaves the pool alongside `public_out`.
        let leaving = public_out
            .checked_add(fee)
            .ok_or(NodeError::BalanceOverflow)?;
        let credited = self
            .balance
            .checked_add(public_in)
            .ok_or(NodeError::BalanceOverflow)?;
        self.balance =
            credited
                .checked_sub(leaving)
                .ok_or(NodeError::ShieldedBalanceUnderflow {
                    held: credited,
                    withdrawn: leaving,
                })?;
        Ok(())
    }

    /// The current tree root.
    #[must_use]
    pub fn root(&self) -> [u8; 32] {
        fr_to_bytes(&self.tree.root())
    }

    /// Number of notes in the pool.
    #[must_use]
    pub fn note_count(&self) -> u64 {
        self.tree.count()
    }

    /// Whether `anchor` is a root this pool will accept a spend against.
    #[must_use]
    pub fn accepts_anchor(&self, anchor: &[u8; 32]) -> bool {
        self.anchors.contains(anchor)
    }

    /// Appends a note commitment.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the commitment is not a canonical field
    /// element, or [`NodeError::ShieldedPoolFull`] once the tree is full.
    pub fn append(&mut self, commitment: &[u8; 32]) -> Result<u64> {
        let element = fr_from_bytes(commitment)
            .map_err(|_| NodeError::Decode("note commitment is not canonical".to_string()))?;
        self.tree
            .append(element)
            .map_err(|_| NodeError::ShieldedPoolFull)
    }

    /// Records the current root as an acceptable anchor, retiring the oldest
    /// once the window is full.
    pub fn seal_anchor(&mut self) {
        let root = self.root();
        if self.anchors.back() == Some(&root) {
            // A block that added no notes leaves the root unchanged; recording
            // it twice would shorten the window for no benefit.
            return;
        }
        if self.anchors.len() == ANCHOR_WINDOW {
            self.anchors.pop_front();
        }
        self.anchors.push_back(root);
    }

    /// Encodes the pool for storage.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + TREE_DEPTH * 33 + 8 + self.anchors.len() * 32);

        buf.extend_from_slice(&self.tree.count().to_le_bytes());
        for slot in self.tree.frontier() {
            match slot {
                Some(node) => {
                    buf.push(1);
                    buf.extend_from_slice(&fr_to_bytes(node));
                }
                None => {
                    buf.push(0);
                    buf.extend_from_slice(&[0u8; 32]);
                }
            }
        }

        buf.extend_from_slice(&self.balance.to_le_bytes());

        buf.extend_from_slice(&(self.anchors.len() as u64).to_le_bytes());
        for anchor in &self.anchors {
            buf.extend_from_slice(anchor);
        }

        buf
    }

    /// Decodes a stored pool.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated, has trailing
    /// bytes, or holds a non-canonical field element.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);

        let count = reader.read_u64()?;
        let mut frontier = [None; TREE_DEPTH];
        for slot in frontier.iter_mut() {
            let present = reader.read_u8()?;
            let node = reader.read_array::<32>()?;
            *slot = match present {
                0 => None,
                1 => Some(fr_from_bytes(&node).map_err(|_| {
                    NodeError::Decode("frontier node is not canonical".to_string())
                })?),
                other => {
                    return Err(NodeError::Decode(format!(
                        "invalid frontier presence flag {other}"
                    )));
                }
            };
        }

        let balance = reader.read_u64()?;

        let anchor_count = reader.read_collection_len(32)?;
        if anchor_count > ANCHOR_WINDOW {
            return Err(NodeError::Decode(format!(
                "anchor window of {anchor_count} exceeds the maximum {ANCHOR_WINDOW}"
            )));
        }
        let mut anchors = VecDeque::with_capacity(anchor_count);
        for _ in 0..anchor_count {
            anchors.push_back(reader.read_array::<32>()?);
        }

        reader.finish()?;

        Ok(Self {
            tree: CommitmentTree::from_parts(frontier, count),
            anchors,
            balance,
        })
    }
}

/// Converts a proved joinsplit into its on-chain form.
///
/// The field-element-to-bytes conversion lives here so there is exactly one
/// place where the prover's view and the chain's view are put in
/// correspondence. Two copies of this mapping that disagreed would produce
/// proofs no node could verify.
#[must_use]
pub fn encode_joinsplit(public: &JoinSplitPublic, proof: [u8; 192]) -> ShieldedJoinSplit {
    ShieldedJoinSplit {
        anchor: fr_to_bytes(&public.anchor),
        nullifiers: [
            fr_to_bytes(&public.nullifiers[0]),
            fr_to_bytes(&public.nullifiers[1]),
        ],
        commitments: [
            fr_to_bytes(&public.commitments[0]),
            fr_to_bytes(&public.commitments[1]),
        ],
        public_in: public.public_in,
        public_out: public.public_out,
        fee: public.fee,
        recipient: public.recipient,
        proof,
    }
}

/// Verifies a joinsplit's zero-knowledge proof.
///
/// This checks only the proof and its internal consistency. Whether the anchor
/// is known and whether the nullifiers are unspent are questions about chain
/// state, and belong to the state transition.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] for non-canonical field elements,
/// [`NodeError::DuplicateNullifier`] if the joinsplit spends one note twice,
/// and [`NodeError::ProofVerification`] if the proof does not hold.
pub fn verify_joinsplit(joinsplit: &ShieldedJoinSplit) -> Result<()> {
    // A joinsplit whose two inputs are the same note would otherwise spend it
    // twice in a single transaction — the circuit has no reason to object, so
    // the check has to live here.
    if joinsplit.nullifiers[0] == joinsplit.nullifiers[1] {
        return Err(NodeError::DuplicateNullifier {
            nullifier: hex::encode(joinsplit.nullifiers[0]),
        });
    }

    let field = |bytes: &[u8; 32], what: &str| {
        fr_from_bytes(bytes)
            .map_err(|_| NodeError::Decode(format!("{what} is not a canonical field element")))
    };

    let public = JoinSplitPublic {
        anchor: field(&joinsplit.anchor, "anchor")?,
        nullifiers: [
            field(&joinsplit.nullifiers[0], "nullifier 0")?,
            field(&joinsplit.nullifiers[1], "nullifier 1")?,
        ],
        commitments: [
            field(&joinsplit.commitments[0], "commitment 0")?,
            field(&joinsplit.commitments[1], "commitment 1")?,
        ],
        public_in: joinsplit.public_in,
        public_out: joinsplit.public_out,
        fee: joinsplit.fee,
        recipient: joinsplit.recipient,
    };

    let accepted = prove::verify(&joinsplit.proof, &public)
        .map_err(|error| NodeError::ProofVerification(error.to_string()))?;

    if accepted {
        Ok(())
    } else {
        Err(NodeError::ProofVerification(
            "the joinsplit proof does not satisfy its public inputs".to_string(),
        ))
    }
}

/// Where fees go.
///
/// Burning them outright would make the pool's effect on total supply
/// unauditable — the one property a shielded pool most needs to keep
/// checkable. Sending them to a fixed sink keeps supply a matter of arithmetic
/// over transparent accounts, matching how transparent transfers already pay.
pub const FEE_SINK: Address = [0u8; 32];

impl StateDB {
    /// Reads the shielded pool, through the overlay if this block has touched
    /// it.
    pub(crate) fn load_pool(&self, overlay: &Overlay) -> Result<ShieldedPool> {
        if let Some(pool) = &overlay.shielded {
            return Ok(pool.clone());
        }
        self.stored_pool()
    }

    /// Reads the committed shielded pool.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure, or
    /// [`NodeError::Decode`] if the stored record is corrupt.
    pub fn stored_pool(&self) -> Result<ShieldedPool> {
        match self.raw_get(POOL_KEY)? {
            Some(bytes) => ShieldedPool::decode(&bytes),
            None => Ok(ShieldedPool::new()),
        }
    }

    /// Whether `nullifier` has already been spent in committed state.
    fn nullifier_committed(&self, nullifier: &[u8; 32]) -> Result<bool> {
        Ok(self.raw_get(&nullifier_key_bytes(nullifier))?.is_some())
    }

    /// Applies a joinsplit to the overlay.
    ///
    /// Order matters. The proof is checked first because it is the only
    /// expensive step that can be decided without state, then the cheap state
    /// questions — is the anchor known, are the nullifiers fresh — and only
    /// then are balances moved.
    pub(crate) fn apply_joinsplit(
        &self,
        overlay: &mut Overlay,
        signer: &Address,
        joinsplit: &ShieldedJoinSplit,
    ) -> Result<()> {
        overlay.joinsplits += 1;
        if overlay.joinsplits > MAX_SHIELDED_PER_BLOCK {
            return Err(NodeError::TooManyShielded {
                actual: overlay.joinsplits,
                limit: MAX_SHIELDED_PER_BLOCK,
            });
        }

        verify_joinsplit(joinsplit)?;

        let mut pool = self.load_pool(overlay)?;

        if !pool.accepts_anchor(&joinsplit.anchor) {
            return Err(NodeError::UnknownAnchor {
                anchor: hex::encode(joinsplit.anchor),
            });
        }

        // Both the committed set and this block's staged spends, so two
        // joinsplits in one block cannot retire the same note.
        for nullifier in &joinsplit.nullifiers {
            if overlay.nullifiers.contains(nullifier) || self.nullifier_committed(nullifier)? {
                return Err(NodeError::NullifierSpent {
                    nullifier: hex::encode(nullifier),
                });
            }
        }
        for nullifier in &joinsplit.nullifiers {
            overlay.nullifiers.insert(*nullifier);
        }

        // Transparent value entering the pool leaves the signer's account.
        if joinsplit.public_in > 0 {
            let mut account = self.account_through(overlay, signer)?;
            if account.balance < joinsplit.public_in {
                return Err(NodeError::InsufficientBalance {
                    address: hex::encode(signer),
                    required: joinsplit.public_in,
                    available: account.balance,
                });
            }
            account.balance -= joinsplit.public_in;
            overlay.accounts.insert(*signer, account);
        }

        if joinsplit.public_out > 0 {
            self.credit_through(overlay, &joinsplit.recipient, joinsplit.public_out)?;
        }
        if joinsplit.fee > 0 {
            self.credit_through(overlay, &FEE_SINK, joinsplit.fee)?;
        }

        for commitment in &joinsplit.commitments {
            pool.append(commitment)?;
        }
        pool.settle(joinsplit.public_in, joinsplit.public_out, joinsplit.fee)?;
        overlay.shielded = Some(pool);

        Ok(())
    }

    /// Reads an account through the overlay.
    fn account_through(&self, overlay: &Overlay, address: &Address) -> Result<Account> {
        match overlay.accounts.get(address) {
            Some(account) => Ok(*account),
            None => self.get_account(address),
        }
    }

    /// Credits an account through the overlay.
    fn credit_through(&self, overlay: &mut Overlay, address: &Address, amount: u64) -> Result<()> {
        let mut account = self.account_through(overlay, address)?;
        account.balance = account
            .balance
            .checked_add(amount)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(*address, account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_pool_accepts_only_the_empty_anchor() {
        let pool = ShieldedPool::new();
        assert!(pool.accepts_anchor(&pool.root()));
        assert!(!pool.accepts_anchor(&[0u8; 32]));
        assert_eq!(pool.note_count(), 0);
    }

    #[test]
    fn appending_advances_the_root() {
        let mut pool = ShieldedPool::new();
        let before = pool.root();
        pool.append(&fr_to_bytes(
            &maya_zk_privacy::note::Note::dummy(1u64.into(), 2u64.into()).commitment(),
        ))
        .expect("append");
        assert_ne!(pool.root(), before);
        assert_eq!(pool.note_count(), 1);
    }

    #[test]
    fn an_old_anchor_stays_valid_inside_the_window() {
        let mut pool = ShieldedPool::new();
        let original = pool.root();

        for value in 0..10u64 {
            pool.append(&fr_to_bytes(
                &maya_zk_privacy::note::Note::dummy((value + 1).into(), (value + 100).into())
                    .commitment(),
            ))
            .expect("append");
            pool.seal_anchor();
        }

        assert!(pool.accepts_anchor(&original));
        assert!(pool.accepts_anchor(&pool.root()));
    }

    #[test]
    fn an_anchor_falls_out_of_the_window_eventually() {
        let mut pool = ShieldedPool::new();
        let original = pool.root();

        for value in 0..(ANCHOR_WINDOW as u64 + 5) {
            pool.append(&fr_to_bytes(
                &maya_zk_privacy::note::Note::dummy((value + 1).into(), (value + 100).into())
                    .commitment(),
            ))
            .expect("append");
            pool.seal_anchor();
        }

        assert!(!pool.accepts_anchor(&original));
        assert!(pool.accepts_anchor(&pool.root()));
    }

    #[test]
    fn a_pool_round_trips_through_storage() {
        let mut pool = ShieldedPool::new();
        for value in 0..7u64 {
            pool.append(&fr_to_bytes(
                &maya_zk_privacy::note::Note::dummy((value + 1).into(), (value + 100).into())
                    .commitment(),
            ))
            .expect("append");
            pool.seal_anchor();
        }

        let restored = ShieldedPool::decode(&pool.encode()).expect("decode");
        assert_eq!(restored, pool);
        assert_eq!(restored.root(), pool.root());
    }

    #[test]
    fn a_truncated_pool_record_is_rejected() {
        let pool = ShieldedPool::new();
        let encoded = pool.encode();
        assert!(ShieldedPool::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let pool = ShieldedPool::new();
        let mut encoded = pool.encode();
        encoded.push(0);
        assert!(ShieldedPool::decode(&encoded).is_err());
    }

    #[test]
    fn a_repeated_root_is_not_recorded_twice() {
        let mut pool = ShieldedPool::new();
        pool.seal_anchor();
        pool.seal_anchor();
        // Only the empty root, recorded once at construction.
        assert_eq!(pool.anchors.len(), 1);
    }
}
