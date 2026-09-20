//! Bidirectionally signed payment channels.
//!
//! ## What makes a state binding
//!
//! A channel state is only enforceable once **both** parties have signed it.
//! One signature is a proposal; two are an agreement. Every update therefore
//! carries two signatures over identical bytes, and [`SignedState::verify`]
//! checks both against the registered participants.
//!
//! ## Revocation
//!
//! Advancing from state *N* to *N+1* requires revealing the revocation secret
//! for *N*. Holding a counterparty's revoked secret is what makes fraud
//! punishable: if they later publish state *N* on chain, the secret proves it
//! was superseded and the whole channel balance is forfeit to the victim.
//!
//! Only the *commitment* ever goes on chain — a hash per state. Publishing the
//! secrets themselves would grow on-chain storage without bound.
//!
//! ## Two signing forms, deliberately
//!
//! - **Update form** commits to `(seq, balances, htlc_root)` and is what parties
//!   exchange off-chain while HTLCs are in flight.
//! - **Settlement form** commits to `(seq, balances)` only, and is the format
//!   the L1 understands.
//!
//! They are different domains, so an update signature can never be replayed as a
//! settlement. A state is only settleable once its HTLC set is empty; otherwise
//! settling would silently discard in-flight value, which is why
//! [`Channel::settlement_closure`] refuses.

use custom_l1_node::core::payload::{
    ChannelClosure, ChannelId, channel_state_signing_bytes, revocation_commitment,
};
use custom_l1_node::crypto::hybrid::{
    HybridPublicKey, HybridSignature, HybridSigningKey, HybridVerifyingKey, address_of,
};

use crate::error::{FlashError, Result};
use crate::htlc::{Direction, Htlc, Preimage, htlc_root};

/// Domain for off-chain state updates. Distinct from the settlement domain so
/// the two signature types cannot be substituted for one another.
const UPDATE_DOMAIN: &[u8] = b"maya-flash.channel-update.v1";

/// A channel participant's address.
pub type Address = [u8; 32];

/// Which participant a value refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Party {
    /// The channel opener.
    A,
    /// The counterparty.
    B,
}

impl Party {
    /// The other participant.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

/// A point-in-time allocation of a channel's capacity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelState {
    /// Channel this state belongs to.
    pub channel_id: ChannelId,
    /// Monotonic sequence number. Higher supersedes lower.
    pub seq: u64,
    /// Value freely spendable by A.
    pub balance_a: u64,
    /// Value freely spendable by B.
    pub balance_b: u64,
    /// In-flight conditional payments, kept sorted by id.
    pub htlcs: Vec<Htlc>,
    /// Commitment to this state's revocation secret.
    ///
    /// Part of the signed state, not a detail bolted on at settlement time: an
    /// unsigned commitment could be swapped by whoever submits the state for
    /// one whose preimage nobody holds, which would quietly disarm the penalty.
    pub revocation_commitment: [u8; 32],
}

impl ChannelState {
    /// Total value the state accounts for: both balances plus every HTLC.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::BalanceOverflow`] on overflow.
    pub fn total(&self) -> Result<u64> {
        let mut total = self
            .balance_a
            .checked_add(self.balance_b)
            .ok_or(FlashError::BalanceOverflow)?;
        for htlc in &self.htlcs {
            total = total
                .checked_add(htlc.amount)
                .ok_or(FlashError::BalanceOverflow)?;
        }
        Ok(total)
    }

    /// The bytes both parties sign for an off-chain update.
    #[must_use]
    pub fn update_signing_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(UPDATE_DOMAIN.len() + 88);
        buf.extend_from_slice(UPDATE_DOMAIN);
        buf.extend_from_slice(&self.channel_id);
        buf.extend_from_slice(&self.seq.to_le_bytes());
        buf.extend_from_slice(&self.balance_a.to_le_bytes());
        buf.extend_from_slice(&self.balance_b.to_le_bytes());
        // Committing to the HTLC set stops either side from agreeing balances
        // while quietly disagreeing about what is still in flight.
        buf.extend_from_slice(&htlc_root(&self.htlcs));
        buf.extend_from_slice(&self.revocation_commitment);
        buf
    }

    /// The bytes both parties sign to settle this state on chain.
    #[must_use]
    pub fn settlement_signing_bytes(&self) -> Vec<u8> {
        channel_state_signing_bytes(
            &self.channel_id,
            self.seq,
            self.balance_a,
            self.balance_b,
            &self.revocation_commitment,
        )
    }

    /// Whether the state can be represented on chain without losing value.
    #[must_use]
    pub fn is_settleable(&self) -> bool {
        self.htlcs.is_empty()
    }

    /// Balance available to `party`.
    #[must_use]
    pub fn balance_of(&self, party: Party) -> u64 {
        match party {
            Party::A => self.balance_a,
            Party::B => self.balance_b,
        }
    }

    fn balance_mut(&mut self, party: Party) -> &mut u64 {
        match party {
            Party::A => &mut self.balance_a,
            Party::B => &mut self.balance_b,
        }
    }
}

/// A channel state carrying both participants' signatures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedState {
    /// The state itself.
    pub state: ChannelState,
    /// A's signature over [`ChannelState::update_signing_bytes`].
    pub sig_a: Box<HybridSignature>,
    /// B's signature over the same bytes.
    pub sig_b: Box<HybridSignature>,
}

impl SignedState {
    /// Verifies both signatures against the registered participants.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::InvalidSignature`] naming the side that failed.
    /// Verified against public keys, not addresses. An address is a hash and
    /// does not yield the keys it commits to, so the caller has to hold them —
    /// [`Channel`] does. Both schemes' proofs must pass for each party; a
    /// counterparty who could forge one half still cannot advance the channel.
    pub fn verify(&self, pubkey_a: &HybridPublicKey, pubkey_b: &HybridPublicKey) -> Result<()> {
        let message = self.state.update_signing_bytes();
        verify_one(pubkey_a, &message, &self.sig_a, "a")?;
        verify_one(pubkey_b, &message, &self.sig_b, "b")
    }
}

fn verify_one(
    public_key: &HybridPublicKey,
    message: &[u8],
    signature: &HybridSignature,
    party: &'static str,
) -> Result<()> {
    let key = HybridVerifyingKey::from_public_key(public_key)
        .map_err(|_| FlashError::InvalidSignature { party })?;
    key.verify(message, signature)
        .map_err(|_| FlashError::InvalidSignature { party })
}

/// A channel as seen by one participant.
#[derive(Clone, Debug)]
pub struct Channel {
    /// Channel identifier.
    pub channel_id: ChannelId,
    /// Party A's address, i.e. the hash of [`Channel::pubkey_a`].
    pub party_a: Address,
    /// Party B's address.
    pub party_b: Address,
    /// Party A's ML-DSA-65 public key.
    ///
    /// Held because an address no longer reveals it, and every state this
    /// channel commits to has to be verified against it. Boxed so a `Channel`
    /// stays a small value to move: two inline keys would put four kilobytes
    /// into every clone.
    pub pubkey_a: Box<HybridPublicKey>,
    /// Party B's ML-DSA-65 public key.
    pub pubkey_b: Box<HybridPublicKey>,
    /// Total funded capacity. Invariant across every valid state.
    pub capacity: u64,
    /// The latest fully signed state.
    pub current: ChannelState,
    /// Revocation secrets received from the counterparty, by sequence.
    ///
    /// Each entry is evidence of fraud should that state ever be published.
    revoked: Vec<(u64, Preimage)>,
    /// Next HTLC id to hand out.
    next_htlc_id: u64,
}

impl Channel {
    /// Opens a channel funded entirely by A.
    #[must_use]
    ///
    /// Takes public keys rather than addresses: the channel must be able to
    /// verify the states its participants sign, and an address cannot do that.
    pub fn open(
        channel_id: ChannelId,
        pubkey_a: Box<HybridPublicKey>,
        pubkey_b: Box<HybridPublicKey>,
        capacity: u64,
    ) -> Self {
        Self {
            channel_id,
            party_a: address_of(&pubkey_a),
            party_b: address_of(&pubkey_b),
            pubkey_a,
            pubkey_b,
            capacity,
            current: ChannelState {
                channel_id,
                seq: 0,
                balance_a: capacity,
                balance_b: 0,
                htlcs: Vec::new(),
                revocation_commitment: [0u8; 32],
            },
            revoked: Vec::new(),
            next_htlc_id: 0,
        }
    }

    /// The address of `party`.
    #[must_use]
    pub fn address_of(&self, party: Party) -> Address {
        match party {
            Party::A => self.party_a,
            Party::B => self.party_b,
        }
    }

    /// Capacity currently spendable from `party` toward the other side.
    #[must_use]
    pub fn spendable(&self, party: Party) -> u64 {
        self.current.balance_of(party)
    }

    /// Checks that a candidate state is a legal successor to the current one.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::StaleSequence`] if the sequence does not advance,
    /// or [`FlashError::CapacityMismatch`] if value is created or destroyed.
    pub fn validate_successor(&self, next: &ChannelState) -> Result<()> {
        if next.seq <= self.current.seq {
            return Err(FlashError::StaleSequence {
                current: self.current.seq,
                proposed: next.seq,
            });
        }

        // Conservation: a channel can never pay out more than was funded, so
        // every state must account for exactly the capacity.
        let total = next.total()?;
        if total != self.capacity {
            return Err(FlashError::CapacityMismatch {
                expected: self.capacity,
                actual: total,
            });
        }

        Ok(())
    }

    /// Builds the next state moving `amount` from `from` to the other side.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::InsufficientBalance`] if the sender's side cannot
    /// cover the amount.
    pub fn propose_transfer(
        &self,
        from: Party,
        amount: u64,
        next_commitment: [u8; 32],
    ) -> Result<ChannelState> {
        let available = self.current.balance_of(from);
        if available < amount {
            return Err(FlashError::InsufficientBalance {
                required: amount,
                available,
            });
        }

        let mut next = self.current.clone();
        next.seq += 1;
        next.revocation_commitment = next_commitment;
        *next.balance_mut(from) = available - amount;
        let to = from.other();
        *next.balance_mut(to) = next
            .balance_of(to)
            .checked_add(amount)
            .ok_or(FlashError::BalanceOverflow)?;

        Ok(next)
    }

    /// Builds the next state adding an HTLC offered by `from`.
    ///
    /// The value leaves the offerer's balance immediately and sits in the HTLC
    /// until it is either fulfilled or refunded — it belongs to neither side in
    /// the meantime.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::InsufficientBalance`] if the offerer cannot cover it.
    pub fn propose_htlc(
        &mut self,
        from: Party,
        amount: u64,
        hash_lock: [u8; 32],
        expiry_height: u64,
        next_commitment: [u8; 32],
    ) -> Result<ChannelState> {
        let available = self.current.balance_of(from);
        if available < amount {
            return Err(FlashError::InsufficientBalance {
                required: amount,
                available,
            });
        }

        let mut next = self.current.clone();
        next.seq += 1;
        next.revocation_commitment = next_commitment;
        *next.balance_mut(from) = available - amount;
        next.htlcs.push(Htlc {
            id: self.next_htlc_id,
            amount,
            hash_lock,
            expiry_height,
            direction: match from {
                Party::A => Direction::AtoB,
                Party::B => Direction::BtoA,
            },
        });
        // Sorted by id so both parties compute the same HTLC root.
        next.htlcs.sort_by_key(|htlc| htlc.id);
        self.next_htlc_id += 1;

        Ok(next)
    }

    /// Builds the next state fulfilling an HTLC with `preimage`.
    ///
    /// The value moves to the receiving side.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::UnknownHtlc`], [`FlashError::PreimageMismatch`],
    /// or [`FlashError::HtlcExpired`] if the claim window has closed.
    pub fn fulfil_htlc(
        &self,
        id: u64,
        preimage: &Preimage,
        height: u64,
        next_commitment: [u8; 32],
    ) -> Result<ChannelState> {
        let htlc = self
            .current
            .htlcs
            .iter()
            .find(|htlc| htlc.id == id)
            .ok_or(FlashError::UnknownHtlc { id })?;

        if !htlc.accepts(preimage) {
            return Err(FlashError::PreimageMismatch { id });
        }
        if htlc.is_expired(height) {
            return Err(FlashError::HtlcExpired {
                id,
                expiry: htlc.expiry_height,
                height,
            });
        }

        let receiver = match htlc.direction {
            Direction::AtoB => Party::B,
            Direction::BtoA => Party::A,
        };
        let amount = htlc.amount;

        let mut next = self.current.clone();
        next.seq += 1;
        next.revocation_commitment = next_commitment;
        next.htlcs.retain(|candidate| candidate.id != id);
        *next.balance_mut(receiver) = next
            .balance_of(receiver)
            .checked_add(amount)
            .ok_or(FlashError::BalanceOverflow)?;

        Ok(next)
    }

    /// Builds the next state refunding an expired HTLC to its offerer.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::UnknownHtlc`], or [`FlashError::HtlcNotExpired`]
    /// if the claim window is still open — refunding early would let the
    /// offerer cancel a payment the receiver can still legitimately claim.
    pub fn refund_htlc(
        &self,
        id: u64,
        height: u64,
        next_commitment: [u8; 32],
    ) -> Result<ChannelState> {
        let htlc = self
            .current
            .htlcs
            .iter()
            .find(|htlc| htlc.id == id)
            .ok_or(FlashError::UnknownHtlc { id })?;

        if !htlc.is_expired(height) {
            return Err(FlashError::HtlcNotExpired {
                id,
                expiry: htlc.expiry_height,
                height,
            });
        }

        let offerer = match htlc.direction {
            Direction::AtoB => Party::A,
            Direction::BtoA => Party::B,
        };
        let amount = htlc.amount;

        let mut next = self.current.clone();
        next.seq += 1;
        next.revocation_commitment = next_commitment;
        next.htlcs.retain(|candidate| candidate.id != id);
        *next.balance_mut(offerer) = next
            .balance_of(offerer)
            .checked_add(amount)
            .ok_or(FlashError::BalanceOverflow)?;

        Ok(next)
    }

    /// Signs a proposed state as `party`.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::InvalidSignature`] if the ML-DSA signer fails,
    /// which FIPS 204 permits when its rejection loop does not terminate.
    pub fn sign(state: &ChannelState, key: &HybridSigningKey) -> Result<Box<HybridSignature>> {
        key.sign(&state.update_signing_bytes())
            .map(Box::new)
            .map_err(|_| FlashError::InvalidSignature { party: "self" })
    }

    /// Adopts a fully signed successor state.
    ///
    /// Records the revocation secret for the state being replaced: that secret
    /// is the only thing that makes the old state's later publication punishable.
    ///
    /// # Errors
    ///
    /// Returns an error if the state is not a legal successor or either
    /// signature fails.
    pub fn commit(&mut self, signed: SignedState, revoked_secret: Preimage) -> Result<()> {
        self.validate_successor(&signed.state)?;
        signed.verify(&self.pubkey_a, &self.pubkey_b)?;

        self.revoked.push((self.current.seq, revoked_secret));
        self.current = signed.state;
        Ok(())
    }

    /// The revocation secret held for `seq`, if the counterparty revealed it.
    #[must_use]
    pub fn revocation_secret(&self, seq: u64) -> Option<Preimage> {
        self.revoked
            .iter()
            .find(|(revoked_seq, _)| *revoked_seq == seq)
            .map(|(_, secret)| *secret)
    }

    /// Commitment published alongside a state, whose preimage punishes its
    /// later republication.
    #[must_use]
    pub fn commitment_for(secret: &Preimage) -> [u8; 32] {
        revocation_commitment(secret)
    }

    /// Builds the on-chain closure for the current state.
    ///
    /// Both parties sign the settlement form, which the L1 verifies against the
    /// channel's registered participants.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::HtlcsPending`] when HTLCs are still in flight:
    /// the on-chain format carries only balances, so settling now would discard
    /// their value.
    pub fn settlement_closure(
        &self,
        key_a: &HybridSigningKey,
        key_b: &HybridSigningKey,
    ) -> Result<ChannelClosure> {
        if !self.current.is_settleable() {
            return Err(FlashError::HtlcsPending {
                pending: self.current.htlcs.len(),
            });
        }

        let message = self.current.settlement_signing_bytes();
        Ok(ChannelClosure {
            channel_id: self.channel_id,
            seq: self.current.seq,
            balance_a: self.current.balance_a,
            balance_b: self.current.balance_b,
            revocation_commitment: self.current.revocation_commitment,
            pubkey_a: self.pubkey_a.clone(),
            pubkey_b: self.pubkey_b.clone(),
            sig_a: Box::new(
                key_a
                    .sign(&message)
                    .map_err(|_| FlashError::InvalidSignature { party: "a" })?,
            ),
            sig_b: Box::new(
                key_b
                    .sign(&message)
                    .map_err(|_| FlashError::InvalidSignature { party: "b" })?,
            ),
        })
    }
}

impl Party {
    /// Human-readable label, for error messages.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.label()
    }
}
