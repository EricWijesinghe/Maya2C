//! One round of secure aggregation (Bonawitz et al., 2017), with ML-KEM in
//! place of Diffie–Hellman.
//!
//! # The round
//!
//! 1. **Advertise.** Each participant publishes an ML-KEM encapsulation key and
//!    a commitment to a fresh self-mask seed.
//! 2. **Agree.** For each pair `i < j`, `i` encapsulates to `j`
//!    ([`Participant::encapsulate`], [`Participant::accept_keys`]).
//! 3. **Share.** Each participant Shamir-shares its self-mask seed, threshold
//!    `t`, sealing one share to each other participant
//!    ([`Participant::share_seed`], [`Participant::receive_shares`]).
//! 4. **Mask.** Each participant sends its quantized, noised update plus its
//!    self-mask plus `+mask` for every higher peer and `-mask` for every lower
//!    one ([`Participant::masked_input`]).
//! 5. **Unmask.** The aggregator names the survivors. Each survivor reveals, for
//!    every other survivor, its share of that survivor's self-mask seed, and for
//!    every dropped participant, the pair seed they shared
//!    ([`Participant::unmask_response`]). The aggregator removes self-masks and
//!    the dropped participants' leftover pair masks, and holds the sum.
//!
//! # What it protects, and against whom
//!
//! The aggregator learns the sum of the survivors' inputs and nothing else, as
//! long as it follows the protocol and fewer than `t` participants collude with
//! it — the honest-but-curious setting. A participant refuses to reveal both
//! halves for anyone: it answers one survivor set per round, and never gives a
//! self-seed share for a participant it was told dropped.
//!
//! Not covered: an aggregator that tells different participants different
//! survivor sets, which the original protocol answers with a signed consistency
//! round; and a survivor dropping *during* unmasking, which here aborts the
//! round. Both are stated rather than implied away.

use std::collections::BTreeMap;

use maya_crypto_pq::kem::{
    CIPHERTEXT_LEN, DecapsulationKey, ENCAPSULATION_KEY_LEN, EncapsulationKey, generate_keypair,
};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::masking::{self, Secret};
use crate::quantize::to_residues;
use crate::random::Randomness;
use crate::shamir::{self, Share};

/// The fixed facts of a round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundConfig {
    round: u64,
    participants: usize,
    threshold: usize,
    dimension: usize,
    max_input: u64,
}

/// Largest magnitude a signed 32-bit aggregate coordinate may reach.
const SIGNED_LIMIT: u64 = 1 << 31;

impl RoundConfig {
    /// A round of `participants` inputs of `dimension` coordinates, each of
    /// magnitude at most `max_input`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidParameter`] unless `2 ≤ participants ≤ 255`, the
    /// threshold is a strict majority and at most `participants`, and the
    /// dimension is non-zero. A majority threshold is what stops two disjoint
    /// groups from each reconstructing a different seed.
    ///
    /// [`Error::Overflow`] unless `participants × max_input` fits a signed
    /// 32-bit coordinate. The sum is taken modulo 2^32 and decoded as signed,
    /// so a bound that could wrap would corrupt the aggregate silently — and the
    /// privacy accounting stated in its units with it. Enforced here and in
    /// [`Participant::masked_input`], not left to the caller.
    pub fn new(
        round: u64,
        participants: usize,
        threshold: usize,
        dimension: usize,
        max_input: u64,
    ) -> Result<Self> {
        let valid = (2..=255).contains(&participants)
            && threshold * 2 > participants
            && threshold <= participants
            && dimension > 0
            && max_input > 0;
        if !valid {
            return Err(Error::InvalidParameter(
                "need 2..=255 participants, a majority threshold, a dimension and an input bound",
            ));
        }
        if (participants as u64)
            .checked_mul(max_input)
            .is_none_or(|total| total >= SIGNED_LIMIT)
        {
            return Err(Error::Overflow {
                nodes: participants,
            });
        }
        Ok(Self {
            round,
            participants,
            threshold,
            dimension,
            max_input,
        })
    }

    /// The largest coordinate magnitude an input may carry.
    #[must_use]
    pub const fn max_input(&self) -> u64 {
        self.max_input
    }

    /// The round number.
    #[must_use]
    pub const fn round(&self) -> u64 {
        self.round
    }

    /// Participants.
    #[must_use]
    pub const fn participants(&self) -> usize {
        self.participants
    }

    /// Survivors needed to unmask.
    #[must_use]
    pub const fn threshold(&self) -> usize {
        self.threshold
    }

    /// Coordinates per input.
    #[must_use]
    pub const fn dimension(&self) -> usize {
        self.dimension
    }

    fn check_index(&self, index: u16) -> Result<()> {
        if usize::from(index) < self.participants {
            Ok(())
        } else {
            Err(Error::UnknownParticipant(index))
        }
    }
}

/// What a participant publishes in step 1.
#[derive(Clone)]
pub struct Advertisement {
    /// The participant.
    pub index: u16,
    /// Its ML-KEM-768 encapsulation key.
    pub encapsulation_key: [u8; ENCAPSULATION_KEY_LEN],
    /// Its commitment to its self-mask seed.
    pub seed_commitment: [u8; 32],
}

/// An ML-KEM ciphertext from `from` to `to`, step 2.
#[derive(Clone)]
pub struct KeyEnvelope {
    /// Lower index: the encapsulator.
    pub from: u16,
    /// Higher index: the decapsulator.
    pub to: u16,
    /// The ciphertext.
    pub ciphertext: [u8; CIPHERTEXT_LEN],
}

/// A sealed seed share from `from` to `to`, step 3.
#[derive(Clone, Debug)]
pub struct ShareEnvelope {
    /// The seed's owner.
    pub from: u16,
    /// The share's holder.
    pub to: u16,
    /// `x` then the share bytes, sealed.
    pub sealed: Vec<u8>,
}

/// A masked input, step 4.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskedInput {
    /// The participant.
    pub index: u16,
    /// Residues modulo 2^32.
    pub values: Vec<u32>,
}

/// A survivor's answer in step 5.
#[derive(Clone, Debug)]
pub struct UnmaskResponse {
    /// The survivor answering.
    pub from: u16,
    /// Its share of each survivor's self-mask seed, by owner.
    pub self_seed_shares: BTreeMap<u16, Share>,
    /// Its pair seed with each dropped participant, by that participant.
    pub dropped_pair_seeds: BTreeMap<u16, [u8; 32]>,
}

/// One participant's secrets for one round.
pub struct Participant {
    config: RoundConfig,
    index: u16,
    decapsulation: DecapsulationKey,
    encapsulation: EncapsulationKey,
    self_seed: Secret,
    rng: Randomness,
    pair_seeds: BTreeMap<u16, Secret>,
    send_keys: BTreeMap<u16, Secret>,
    receive_keys: BTreeMap<u16, Secret>,
    held_shares: BTreeMap<u16, Share>,
    own_share: Option<Share>,
    answered: Option<Vec<u16>>,
}

impl Participant {
    /// A participant with fresh keys, drawing its seeds from `entropy`.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownParticipant`] for an index outside the round.
    pub fn new(config: RoundConfig, index: u16, entropy: &[u8; 32]) -> Result<Self> {
        config.check_index(index)?;
        let (decapsulation, encapsulation) = generate_keypair();
        let mut rng = Randomness::from_seed(entropy, "maya2c confidential-ai participant v1");
        let mut seed = Zeroizing::new([0u8; 32]);
        rng.fill(seed.as_mut());
        Ok(Self {
            config,
            index,
            decapsulation,
            encapsulation,
            self_seed: seed,
            rng,
            pair_seeds: BTreeMap::new(),
            send_keys: BTreeMap::new(),
            receive_keys: BTreeMap::new(),
            held_shares: BTreeMap::new(),
            own_share: None,
            answered: None,
        })
    }

    /// This participant's index.
    #[must_use]
    pub const fn index(&self) -> u16 {
        self.index
    }

    /// Step 1.
    #[must_use]
    pub fn advertise(&self) -> Advertisement {
        Advertisement {
            index: self.index,
            encapsulation_key: self.encapsulation.to_bytes(),
            seed_commitment: masking::seed_commitment(&self.self_seed),
        }
    }

    fn remember(&mut self, peer: u16, secret: &maya_crypto_pq::kem::SharedSecret) {
        let round = self.config.round;
        self.pair_seeds
            .insert(peer, masking::pair_seed(secret, round, self.index, peer));
        self.send_keys
            .insert(peer, masking::share_key(secret, round, self.index, peer));
        self.receive_keys
            .insert(peer, masking::share_key(secret, round, peer, self.index));
    }

    /// Step 2, sending half: encapsulates to every higher-indexed peer.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidParameter`] for a malformed encapsulation key.
    pub fn encapsulate(&mut self, advertisements: &[Advertisement]) -> Result<Vec<KeyEnvelope>> {
        let mut envelopes = Vec::new();
        let me = self.index;
        for advertisement in advertisements.iter().filter(|a| a.index > me) {
            self.config.check_index(advertisement.index)?;
            let key = EncapsulationKey::from_bytes(&advertisement.encapsulation_key)
                .map_err(|_| Error::InvalidParameter("malformed encapsulation key"))?;
            let (ciphertext, secret) = key.encapsulate();
            self.remember(advertisement.index, &secret);
            envelopes.push(KeyEnvelope {
                from: self.index,
                to: advertisement.index,
                ciphertext,
            });
        }
        Ok(envelopes)
    }

    /// Step 2, receiving half: decapsulates every ciphertext addressed here.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownParticipant`] for a sender outside the round.
    pub fn accept_keys(&mut self, envelopes: &[KeyEnvelope]) -> Result<()> {
        let me = self.index;
        for envelope in envelopes.iter().filter(|e| e.to == me) {
            self.config.check_index(envelope.from)?;
            let secret = self.decapsulation.decapsulate(&envelope.ciphertext);
            self.remember(envelope.from, &secret);
        }
        Ok(())
    }

    /// Step 3, sending half: shares the self-mask seed, one sealed share per peer.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidParameter`] if key agreement is incomplete.
    pub fn share_seed(&mut self) -> Result<Vec<ShareEnvelope>> {
        let shares = shamir::split(
            self.self_seed.as_ref(),
            self.config.threshold,
            self.config.participants,
            &mut self.rng,
        )?;
        let mut envelopes = Vec::new();
        for (holder, share) in shares.into_iter().enumerate() {
            let holder = holder as u16;
            if holder == self.index {
                self.own_share = Some(share);
                continue;
            }
            let key = self
                .send_keys
                .get(&holder)
                .ok_or(Error::InvalidParameter("key agreement is incomplete"))?;
            let mut plaintext = Zeroizing::new(Vec::with_capacity(33));
            plaintext.push(share.x);
            plaintext.extend_from_slice(&share.y);
            envelopes.push(ShareEnvelope {
                from: self.index,
                to: holder,
                sealed: masking::seal(key, self.config.round, self.index, holder, &plaintext)?,
            });
        }
        Ok(envelopes)
    }

    /// Step 3, receiving half: opens and holds every share addressed here.
    ///
    /// # Errors
    ///
    /// [`Error::AuthenticationFailed`] for a share that does not open.
    pub fn receive_shares(&mut self, envelopes: &[ShareEnvelope]) -> Result<()> {
        let me = self.index;
        for envelope in envelopes.iter().filter(|e| e.to == me) {
            let key = self
                .receive_keys
                .get(&envelope.from)
                .ok_or(Error::UnknownParticipant(envelope.from))?;
            let opened = masking::open(
                key,
                self.config.round,
                envelope.from,
                self.index,
                &envelope.sealed,
            )?;
            let (&x, y) = opened.split_first().ok_or(Error::AuthenticationFailed)?;
            self.held_shares
                .insert(envelope.from, Share { x, y: y.to_vec() });
        }
        Ok(())
    }

    /// Step 4: the masked form of an already quantized and noised update.
    ///
    /// # Errors
    ///
    /// [`Error::DimensionMismatch`] for the wrong length; [`Error::InvalidParameter`]
    /// if key agreement is incomplete.
    pub fn masked_input(&self, update: &[i64]) -> Result<MaskedInput> {
        if update.len() != self.config.dimension {
            return Err(Error::DimensionMismatch {
                expected: self.config.dimension,
                found: update.len(),
            });
        }
        if self.pair_seeds.len() + 1 != self.config.participants {
            return Err(Error::InvalidParameter("key agreement is incomplete"));
        }
        if update
            .iter()
            .any(|value| value.unsigned_abs() > self.config.max_input)
        {
            return Err(Error::Overflow {
                nodes: self.config.participants,
            });
        }
        let mut values = to_residues(update);
        let dimension = values.len();
        masking::apply(
            &mut values,
            &masking::expand(&self.self_seed, dimension),
            true,
        );
        for (peer, seed) in &self.pair_seeds {
            let mask = masking::expand(seed, values.len());
            masking::apply(&mut values, &mask, self.index < *peer);
        }
        Ok(MaskedInput {
            index: self.index,
            values,
        })
    }

    /// Step 5: this survivor's answer for `survivors`.
    ///
    /// # Errors
    ///
    /// [`Error::BelowThreshold`] for too few survivors; [`Error::InvalidParameter`]
    /// if this participant is not among them, or already answered for a
    /// different set — answering twice is how an aggregator would learn both
    /// halves for somebody.
    pub fn unmask_response(&mut self, survivors: &[u16]) -> Result<UnmaskResponse> {
        let mut survivors = survivors.to_vec();
        survivors.sort_unstable();
        survivors.dedup();
        if survivors.len() < self.config.threshold {
            return Err(Error::BelowThreshold {
                survivors: survivors.len(),
                threshold: self.config.threshold,
            });
        }
        if !survivors.contains(&self.index) {
            return Err(Error::InvalidParameter(
                "a dropped participant does not answer",
            ));
        }
        if self
            .answered
            .as_ref()
            .is_some_and(|previous| *previous != survivors)
        {
            return Err(Error::InvalidParameter(
                "already answered for a different survivor set",
            ));
        }

        let mut self_seed_shares = BTreeMap::new();
        for &survivor in &survivors {
            let share = if survivor == self.index {
                self.own_share.clone()
            } else {
                self.held_shares.get(&survivor).cloned()
            };
            let share = share.ok_or(Error::InvalidParameter("seed sharing is incomplete"))?;
            self_seed_shares.insert(survivor, share);
        }
        let dropped_pair_seeds = self
            .pair_seeds
            .iter()
            .filter(|(peer, _)| survivors.binary_search(peer).is_err())
            .map(|(peer, seed)| (*peer, **seed))
            .collect();
        self.answered = Some(survivors);
        Ok(UnmaskResponse {
            from: self.index,
            self_seed_shares,
            dropped_pair_seeds,
        })
    }
}

/// The aggregator: collects masked inputs and unmasks their sum.
pub struct Aggregator {
    config: RoundConfig,
    commitments: BTreeMap<u16, [u8; 32]>,
    inputs: BTreeMap<u16, Vec<u32>>,
}

impl Aggregator {
    /// An aggregator for the advertised participants.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidParameter`] unless every index in the round advertised
    /// exactly once.
    pub fn new(config: RoundConfig, advertisements: &[Advertisement]) -> Result<Self> {
        let commitments: BTreeMap<u16, [u8; 32]> = advertisements
            .iter()
            .map(|a| (a.index, a.seed_commitment))
            .collect();
        let complete = commitments.len() == config.participants
            && advertisements.len() == config.participants
            && commitments
                .keys()
                .all(|&index| config.check_index(index).is_ok());
        if !complete {
            return Err(Error::InvalidParameter(
                "every participant advertises exactly once",
            ));
        }
        Ok(Self {
            config,
            commitments,
            inputs: BTreeMap::new(),
        })
    }

    /// Accepts one masked input.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownParticipant`], [`Error::DuplicateSubmission`], or
    /// [`Error::DimensionMismatch`].
    pub fn submit(&mut self, input: MaskedInput) -> Result<()> {
        self.config.check_index(input.index)?;
        if input.values.len() != self.config.dimension {
            return Err(Error::DimensionMismatch {
                expected: self.config.dimension,
                found: input.values.len(),
            });
        }
        if self.inputs.contains_key(&input.index) {
            return Err(Error::DuplicateSubmission(input.index));
        }
        self.inputs.insert(input.index, input.values);
        Ok(())
    }

    /// The participants whose inputs arrived, ascending.
    #[must_use]
    pub fn survivors(&self) -> Vec<u16> {
        self.inputs.keys().copied().collect()
    }

    /// Removes every mask and returns the survivors' sum.
    ///
    /// # Errors
    ///
    /// [`Error::BelowThreshold`] for too few survivors or a missing survivor
    /// response; [`Error::CommitmentMismatch`] for a seed that does not match
    /// its commitment; [`Error::InvalidParameter`] for a missing pair seed.
    pub fn unmask(&self, responses: &[UnmaskResponse]) -> Result<Vec<u32>> {
        let survivors = self.survivors();
        let by_survivor: BTreeMap<u16, &UnmaskResponse> = responses
            .iter()
            .filter(|r| self.inputs.contains_key(&r.from))
            .map(|r| (r.from, r))
            .collect();
        if survivors.len() < self.config.threshold || by_survivor.len() != survivors.len() {
            return Err(Error::BelowThreshold {
                survivors: by_survivor.len().min(survivors.len()),
                threshold: self.config.threshold,
            });
        }

        let mut sum = vec![0u32; self.config.dimension];
        for values in self.inputs.values() {
            masking::apply(&mut sum, values, true);
        }
        for &survivor in &survivors {
            let shares: Vec<Share> = by_survivor
                .values()
                .filter_map(|r| r.self_seed_shares.get(&survivor).cloned())
                .collect();
            let recovered = shamir::combine(&shares, self.config.threshold)?;
            let seed: [u8; 32] = recovered
                .as_slice()
                .try_into()
                .map_err(|_| Error::CommitmentMismatch(survivor))?;
            if self.commitments.get(&survivor) != Some(&masking::seed_commitment(&seed)) {
                return Err(Error::CommitmentMismatch(survivor));
            }
            masking::apply(
                &mut sum,
                &masking::expand(&seed, self.config.dimension),
                false,
            );
        }
        let dropped = (0..self.config.participants as u16).filter(|i| !self.inputs.contains_key(i));
        for lost in dropped {
            for (&survivor, response) in &by_survivor {
                let seed =
                    response
                        .dropped_pair_seeds
                        .get(&lost)
                        .ok_or(Error::InvalidParameter(
                            "a survivor omitted a dropped pair seed",
                        ))?;
                // The survivor added the mask if it had the lower index.
                let mask = masking::expand(seed, self.config.dimension);
                masking::apply(&mut sum, &mask, survivor > lost);
            }
        }
        Ok(sum)
    }

    /// A digest of the round an attestation report must bind: the round, every
    /// commitment, the survivors and the unmasked sum.
    #[must_use]
    pub fn transcript(&self, sum: &[u32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya2c confidential-ai transcript v1");
        hasher.update(&self.config.round.to_le_bytes());
        for (index, commitment) in &self.commitments {
            hasher.update(&index.to_le_bytes());
            hasher.update(commitment);
        }
        for survivor in self.inputs.keys() {
            hasher.update(&survivor.to_le_bytes());
        }
        for value in sum {
            hasher.update(&value.to_le_bytes());
        }
        *hasher.finalize().as_bytes()
    }
}
