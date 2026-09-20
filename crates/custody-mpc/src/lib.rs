//! Threshold custody of a Maya2C chain key.
//!
//! # Read this paragraph before the rest
//!
//! **This is not a threshold signature scheme.** A quorum's shares are
//! reconstructed in one place to produce a signature. For the length of one
//! signature, one machine can sign anything the vault owns. If that is not
//! acceptable for a given deployment, nothing in this crate makes it
//! acceptable, and no amount of reading further will change that.
//!
//! What it *is*: a vault key that is generated with no dealer, held by nobody,
//! split so that `t-1` custodians learn nothing about it in the
//! information-theoretic sense, and reassembled only for as long as it takes to
//! sign — with every step from generation to reconstruction checkable against
//! public commitments.
//!
//! # Why not a real TSS
//!
//! Because a Maya2C signature is a hybrid pair and both halves must verify
//! (`src/crypto/hybrid.rs:337`):
//!
//! | Half | Scheme | Threshold construction available? |
//! |---|---|---|
//! | lattice | ML-DSA-65, FIPS 204 | Only since 2025, unstandardised, no production implementation, and none in `fips204` |
//! | hash-based | SLH-DSA-SHA2-128s, FIPS 205 | **None exists** |
//!
//! A scheme that thresholds one half produces something no node accepts. The
//! citations and the full argument are in `docs/custody-mpc.md`.
//!
//! # What makes the problem tractable anyway
//!
//! A whole Maya2C identity is **32 bytes**: `signing_key_from_seed`
//! (`src/crypto/hybrid.rs:469`) derives both halves of the hybrid pair from one
//! chain key. So protecting 32 bytes protects both schemes at once, and the
//! problem becomes threshold custody of a small secret rather than threshold
//! signing under two incompatible signature algorithms.
//!
//! # The shape of it
//!
//! | Module | What it owns |
//! |---|---|
//! | [`vss`] | Pedersen verifiable secret sharing over Ristretto |
//! | [`seal`] | ML-KEM-768 sealing, so a share is opaque to its transport |
//! | [`dkg`] | The dealerless ceremony: announce, deal, verify, sum |
//! | [`session`] | The quorum that signs, and the checks it runs first |
//! | [`hybrid`] | Chain key to hybrid signing key, mirroring the node |
//! | [`transport`] | The wire format |
//! | [`tls`] | Mutually-authenticated TLS, behind the `tls` feature |
//!
//! # A worked 3-of-5
//!
//! ```
//! use maya_custody_mpc::dkg::{Custodian, Roster, VaultPolicy};
//! use maya_custody_mpc::session::{SigningSession, VaultDescriptor};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let policy = VaultPolicy::new(3, 5)?;
//!
//! // Round one: everybody announces.
//! let custodians: Vec<_> = (1..=5)
//!     .map(|i| Custodian::begin(policy, i))
//!     .collect::<Result<_, _>>()?;
//! let announcements: Vec<_> = custodians.iter().map(Custodian::announce).collect();
//! let roster = Roster::assemble(policy, &announcements)?;
//!
//! // Round two: everybody deals. Nobody has drawn the vault key, because
//! // there is no vault key to draw -- it is the sum of five contributions.
//! let dealings: Vec<_> = custodians
//!     .iter()
//!     .map(|c| c.deal(&roster))
//!     .collect::<Result<_, _>>()?;
//!
//! // Round three: everybody verifies and sums.
//! let held: Vec<_> = custodians
//!     .iter()
//!     .map(|c| c.accept(&roster, &dealings))
//!     .collect::<Result<_, _>>()?;
//!
//! // Three of the five convene. Custodians 2 and 4 are offline.
//! let vault = VaultDescriptor::establish(&[held[0].clone(), held[2].clone(), held[4].clone()])?;
//!
//! let mut session = SigningSession::open(vault.clone(), b"a transaction".to_vec());
//! session.contribute(&held[0])?;
//! session.contribute(&held[1])?;
//! session.contribute(&held[3])?;
//! let signature = session.sign()?;
//!
//! assert_eq!(signature.len(), 11_165);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod dkg;
pub mod error;
pub mod hybrid;
pub mod seal;
pub mod session;
pub mod transport;
pub mod vss;

#[cfg(feature = "tls")]
pub mod tls;

pub use error::{CustodyError, Result};
