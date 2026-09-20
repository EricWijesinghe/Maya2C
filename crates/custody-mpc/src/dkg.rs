//! Distributed key generation with no dealer.
//!
//! # What "without assembling the full key" actually means here
//!
//! Every custodian draws its own contribution and verifiably shares it with
//! everybody. The vault secret is the sum of all `n` contributions; each
//! custodian's share of it is the sum of the `n` shares it received. Nobody
//! ever holds the sum, no machine ever computes it, and there is no point in
//! the ceremony at which a key exists to be stolen. That is a real property and
//! it is what this module delivers.
//!
//! It rests on one line of arithmetic: secret sharing is linear, so the sum of
//! shares of `n` secrets is a share of the sum. [`vss::Commitments::add`] is
//! the same statement about the public half.
//!
//! # What it does not mean
//!
//! It does **not** mean the key never assembles. It means it never assembles
//! *during generation*. To sign, a quorum's shares are reconstructed in one
//! place — see [`crate::session`], which says so at length. A ceremony that
//! avoids a dealer and a protocol that avoids a combiner are different
//! problems, and this crate solves only the first.
//!
//! # The rounds
//!
//! | Round | Every custodian | Public |
//! |---|---|---|
//! | 1 | draws a ceremony KEM key | its [`Announcement`] |
//! | — | — | the [`Roster`], which fixes the [`VaultId`] |
//! | 2 | verifiably shares one contribution | its [`Dealing`]: commitments and `n` sealed shares |
//! | 3 | opens, **verifies**, and sums | the vault's summed [`vss::Commitments`] |
//!
//! Round 3 is where a dishonest dealer is caught, by every recipient
//! independently, using nothing but public data. A dealing that fails names its
//! dealer.

use zeroize::Zeroizing;

use crate::error::{CustodyError, Result};
use crate::seal::{self, CeremonyKey, SealedShare};
use crate::vss::{self, Commitments, ShareBody};
use maya_crypto_pq::kem::ENCAPSULATION_KEY_LEN;

/// Domain string for the vault identifier.
const VAULT_ID_DOMAIN: &[u8] = b"maya2c.custody-mpc.vault-id.v1";

/// How many custodians a vault may have.
///
/// Bounded by the share index being one byte, and by index `0` being the
/// secret's own position rather than a custodian's. Not a tuning parameter:
/// widening it changes the wire format.
pub const MAX_CUSTODIANS: u8 = u8::MAX - 1;

/// The `t`-of-`n` rule a vault is built to.
///
/// The fields are not public: the only way to get a policy is
/// [`VaultPolicy::new`], so every policy in existence has passed its
/// satisfiability check. With public fields that check was opt-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultPolicy {
    pub(crate) threshold: u8,
    pub(crate) custodians: u8,
}

impl VaultPolicy {
    /// Builds a policy, checking it is satisfiable.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::EmptyVault`] for no custodians.
    /// - [`CustodyError::InvalidThreshold`] unless `1 <= threshold <= custodians`,
    ///   or if the roster is larger than [`MAX_CUSTODIANS`].
    pub fn new(threshold: u8, custodians: u8) -> Result<Self> {
        if custodians == 0 {
            return Err(CustodyError::EmptyVault);
        }
        if threshold == 0 || threshold > custodians || custodians > MAX_CUSTODIANS {
            return Err(CustodyError::InvalidThreshold {
                threshold,
                custodians,
            });
        }
        Ok(Self {
            threshold,
            custodians,
        })
    }

    /// How many custodians must agree to sign.
    #[must_use]
    pub fn threshold(&self) -> u8 {
        self.threshold
    }

    /// How many custodians hold shares.
    #[must_use]
    pub fn custodians(&self) -> u8 {
        self.custodians
    }
}

/// A vault's identifier: a digest over its policy and its whole roster.
///
/// Bound into every commitment check and every sealed share. A share from last
/// quarter's vault is then rejected on arrival rather than folded into this
/// quarter's, which is the difference between an error message and an
/// unexplained bad reconstruction six months later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VaultId(pub [u8; 32]);

/// What a custodian publishes in round one.
#[derive(Clone)]
pub struct Announcement {
    /// This custodian's index, in `1..=custodians`.
    pub index: u8,
    /// The ceremony KEM key others seal this custodian's shares to.
    pub encapsulation_key: [u8; ENCAPSULATION_KEY_LEN],
}

impl core::fmt::Debug for Announcement {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Announcement")
            .field("index", &self.index)
            .field("encapsulation_key", &"<1184 bytes>")
            .finish()
    }
}

/// The agreed roster, and the vault identity it fixes.
///
/// Every custodian builds this independently from the same announcements and
/// must arrive at the same [`VaultId`]. Two custodians with different rosters
/// produce different ids and every subsequent message between them is rejected
/// — which is the intended outcome, because they are building different vaults.
#[derive(Clone, Debug)]
pub struct Roster {
    /// The policy.
    pub policy: VaultPolicy,
    /// Announcements, ordered by index `1..=custodians`.
    pub members: Vec<Announcement>,
    /// The identifier this roster fixes.
    pub id: VaultId,
}

impl Roster {
    /// Orders and checks a set of announcements, then derives the vault id.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::ReservedIndex`] if any announcement claims index `0`.
    /// - [`CustodyError::UnknownCustodian`] for an index above the roster.
    /// - [`CustodyError::DuplicateContribution`] if two announcements share an
    ///   index.
    /// - [`CustodyError::MissingDealer`] if the roster is not complete. A vault
    ///   built from a partial roster is a vault whose absent members hold shares
    ///   of nothing.
    pub fn assemble(policy: VaultPolicy, announcements: &[Announcement]) -> Result<Self> {
        let mut slots: Vec<Option<Announcement>> = vec![None; usize::from(policy.custodians)];
        for announcement in announcements {
            if announcement.index == 0 {
                return Err(CustodyError::ReservedIndex);
            }
            if announcement.index > policy.custodians {
                return Err(CustodyError::UnknownCustodian {
                    index: announcement.index,
                    custodians: policy.custodians,
                });
            }
            let slot = &mut slots[usize::from(announcement.index) - 1];
            if slot.is_some() {
                return Err(CustodyError::DuplicateContribution(announcement.index));
            }
            *slot = Some(announcement.clone());
        }

        let mut members = Vec::with_capacity(usize::from(policy.custodians));
        for (position, slot) in slots.into_iter().enumerate() {
            let index = u8::try_from(position + 1).expect("roster is bounded by MAX_CUSTODIANS");
            members.push(slot.ok_or(CustodyError::MissingDealer(index))?);
        }

        // The id covers the policy and every encapsulation key, in index order.
        // Covering the keys and not merely the count is what makes a substituted
        // custodian a different vault rather than the same vault with a new
        // member.
        let mut hasher = blake3::Hasher::new();
        hasher.update(VAULT_ID_DOMAIN);
        hasher.update(&[policy.threshold, policy.custodians]);
        for member in &members {
            hasher.update(&[member.index]);
            hasher.update(&member.encapsulation_key);
        }
        let id = VaultId(*hasher.finalize().as_bytes());

        Ok(Self {
            policy,
            members,
            id,
        })
    }

    /// The announcement at `index`.
    fn member(&self, index: u8) -> Result<&Announcement> {
        self.members
            .get(usize::from(index).wrapping_sub(1))
            .ok_or(CustodyError::UnknownCustodian {
                index,
                custodians: self.policy.custodians,
            })
    }
}

/// One custodian's round-two output: what it commits to, and what it dealt.
#[derive(Clone, Debug)]
pub struct Dealing {
    /// Who dealt it.
    pub dealer: u8,
    /// The commitments to this dealer's polynomial.
    pub commitments: Commitments,
    /// One sealed share per custodian, including one for the dealer itself.
    pub sealed: Vec<SealedShare>,
}

/// A custodian's long-lived holding: its share of the vault secret.
///
/// This is the thing an institution puts in an HSM. It is worth nothing on its
/// own — `threshold - 1` of these reveal, in the information-theoretic sense,
/// nothing about the vault key — and everything in combination.
#[derive(Clone)]
pub struct CustodianShare {
    /// Which vault.
    pub vault: VaultId,
    /// The policy, carried so a share is self-describing in a safe.
    pub policy: VaultPolicy,
    /// This custodian's index.
    pub index: u8,
    /// The share itself.
    pub share: ShareBody,
    /// The vault's summed commitments — public, and the reference every
    /// reconstruction is checked against.
    pub commitments: Commitments,
}

impl core::fmt::Debug for CustodianShare {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CustodianShare")
            .field("vault", &self.vault)
            .field("policy", &self.policy)
            .field("index", &self.index)
            .field("share", &self.share)
            .finish()
    }
}

/// One custodian's private state for the duration of a ceremony.
pub struct Custodian {
    index: u8,
    key: CeremonyKey,
}

impl core::fmt::Debug for Custodian {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Custodian")
            .field("index", &self.index)
            .field("key", &self.key)
            .finish()
    }
}

impl Custodian {
    /// Starts a ceremony as custodian `index`.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::ReservedIndex`] for index `0`.
    /// - [`CustodyError::UnknownCustodian`] for an index above the roster.
    pub fn begin(policy: VaultPolicy, index: u8) -> Result<Self> {
        if index == 0 {
            return Err(CustodyError::ReservedIndex);
        }
        if index > policy.custodians {
            return Err(CustodyError::UnknownCustodian {
                index,
                custodians: policy.custodians,
            });
        }
        Ok(Self {
            index,
            key: CeremonyKey::generate(),
        })
    }

    /// This custodian's index.
    #[must_use]
    pub fn index(&self) -> u8 {
        self.index
    }

    /// Round one: what to publish.
    #[must_use]
    pub fn announce(&self) -> Announcement {
        Announcement {
            index: self.index,
            encapsulation_key: self.key.encapsulation_key(),
        }
    }

    /// Round two: draw a contribution and verifiably share it.
    ///
    /// The contribution is drawn here and dropped when this returns. It is
    /// never stored, never returned, and never sent — only its shares and its
    /// commitments leave the function, which is what makes the dealer as
    /// ignorant of the vault secret afterwards as everyone else.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::EntropyFailure`] if the OS entropy source is
    ///   unavailable.
    /// - [`CustodyError::SealFailed`] if a roster member's encapsulation key
    ///   does not decode.
    pub fn deal(&self, roster: &Roster) -> Result<Dealing> {
        let contribution = Zeroizing::new(vss::random_scalar()?);
        let (commitments, shares) = vss::deal(
            &contribution,
            roster.policy.threshold,
            roster.policy.custodians,
        )?;

        let mut sealed = Vec::with_capacity(shares.len());
        for share in &shares {
            let member = roster.member(share.index)?;
            sealed.push(seal::seal(
                share,
                &member.encapsulation_key,
                &roster.id.0,
                self.index,
            )?);
        }

        Ok(Dealing {
            dealer: self.index,
            commitments,
            sealed,
        })
    }

    /// Round three: open every dealing addressed here, verify it, and sum.
    ///
    /// Every dealing must be present. Accepting a subset would produce a
    /// custodian holding a share of a *different* secret from everyone who
    /// accepted the full set, and the two only discover it at signing time.
    ///
    /// # Errors
    ///
    /// - [`CustodyError::MissingDealer`] if a roster member dealt nothing.
    /// - [`CustodyError::DuplicateContribution`] if a dealer dealt twice.
    /// - [`CustodyError::MalformedCommitment`] if a commitment vector is not
    ///   `threshold` long. A shorter vector is a lower threshold smuggled into
    ///   one member's dealing.
    /// - [`CustodyError::SealFailed`] if a share does not open.
    /// - [`CustodyError::InconsistentShare`] if an opened share does not satisfy
    ///   its dealer's commitments — the dishonest-dealer case, naming the dealer.
    pub fn accept(&self, roster: &Roster, dealings: &[Dealing]) -> Result<CustodianShare> {
        let expected = usize::from(roster.policy.threshold);
        let mut seen = vec![false; usize::from(roster.policy.custodians)];

        let mut value = Zeroizing::new(curve25519_dalek::scalar::Scalar::ZERO);
        let mut blind = Zeroizing::new(curve25519_dalek::scalar::Scalar::ZERO);
        let mut commitments = Commitments::identity(expected);

        for dealing in dealings {
            let dealer = dealing.dealer;
            let slot = seen.get_mut(usize::from(dealer).wrapping_sub(1)).ok_or(
                CustodyError::UnknownCustodian {
                    index: dealer,
                    custodians: roster.policy.custodians,
                },
            )?;
            if *slot {
                return Err(CustodyError::DuplicateContribution(dealer));
            }
            *slot = true;

            if dealing.commitments.len() != expected {
                return Err(CustodyError::MalformedCommitment {
                    dealer,
                    found: dealing.commitments.len(),
                    expected,
                });
            }

            let mine = dealing
                .sealed
                .iter()
                .find(|s| s.recipient == self.index)
                .ok_or(CustodyError::InconsistentShare {
                    dealer,
                    recipient: self.index,
                })?;

            let share = seal::open(mine, &self.key, &roster.id.0)?;
            // A dealer could seal a share carrying somebody else's index. The
            // AEAD's additional data already binds the recipient, so this is
            // belt and braces — but the failure it prevents is a share that
            // interpolates at the wrong point, which nothing downstream detects.
            if share.index != self.index {
                return Err(CustodyError::InconsistentShare {
                    dealer,
                    recipient: self.index,
                });
            }
            vss::verify(&share, &dealing.commitments, dealer)?;

            *value += share.value;
            *blind += share.blind;
            commitments = commitments.add(&dealing.commitments, dealer)?;
        }

        for (position, present) in seen.iter().enumerate() {
            if !present {
                let index = u8::try_from(position + 1).expect("roster is bounded");
                return Err(CustodyError::MissingDealer(index));
            }
        }

        Ok(CustodianShare {
            vault: roster.id,
            policy: roster.policy,
            index: self.index,
            share: ShareBody {
                index: self.index,
                value: *value,
                blind: *blind,
            },
            commitments,
        })
    }
}
