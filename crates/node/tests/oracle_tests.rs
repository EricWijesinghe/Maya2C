//! The oracle against real block execution.
//!
//! `crates/vrf/tests/` proves the VRF conforms to RFC 9381. These cover everything the
//! VRF deliberately does not know about: who is allowed to propose, what
//! happens when nobody does, whether a quorum is a count of *distinct*
//! authorities, and whether a contract can read a stale price by forgetting to
//! check.
//!
//! Three of these are load-bearing rather than merely nice:
//!
//! - **A liar in the quorum must not move the median.** That is the entire
//!   security argument for believing a feed at all.
//! - **A missing beacon proof must not stall the chain.** Otherwise one
//!   authority going offline halts the network.
//! - **A stale feed must be unreadable, not merely marked stale.** A contract
//!   author who forgets a check is the failure mode most oracle post-mortems
//!   are made of, so the ABI has to make forgetting impossible.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use custom_l1_node::core::oracle_payload::{
    BeaconSubmission, FeedCreation, FeedObservation, FeedSubmission, RegistryRotation,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::oracle::beacon::{BeaconState, beacon_alpha, fallback_beacon};
use custom_l1_node::oracle::feed::{FEED_NAME_LEN, derive_feed_id, observation_bytes};
use custom_l1_node::oracle::registry::{OracleAuthority, OracleRegistry};
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_vrf::ecvrf::prove;
use maya_vrf::keys::VrfSecretKey;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// Five authorities, quorum three: the smallest set where a single liar is
/// outvoted and a median has two values on each side of it.
const AUTHORITIES: usize = 5;
const QUORUM: u8 = 3;
const CHAIN_ID: &str = "maya-oracle-tests";

struct Authority {
    signing: HybridSigningKey,
    vrf: VrfSecretKey,
    address: Address,
}

struct Fixture {
    db: StateDB,
    authorities: Vec<Authority>,
    _dir: TempDir,
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

fn signed(kind: TxKind, nonce: u64, key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::with_kind(kind, nonce);
    tx.sign(key).expect("sign");
    tx
}

fn feed_name(text: &str) -> [u8; FEED_NAME_LEN] {
    let mut out = [0u8; FEED_NAME_LEN];
    out[..text.len()].copy_from_slice(text.as_bytes());
    out
}

/// A chain with a seeded authority set and one funded account per authority.
fn fixture() -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");

    let authorities: Vec<Authority> = (0..AUTHORITIES)
        .map(|index| {
            let signing = generate_signing_key().expect("key");
            let address = signing.address();
            // Distinct, deterministic VRF seeds, so a failure names one
            // authority rather than "one of five".
            let vrf = VrfSecretKey::from_seed([index as u8 + 1; 32]);
            Authority {
                signing,
                vrf,
                address,
            }
        })
        .collect();

    for authority in &authorities {
        db.put_account(
            &authority.address,
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .expect("fund");
    }

    let registry = OracleRegistry::new(
        authorities
            .iter()
            .map(|authority| OracleAuthority {
                address: authority.address,
                vrf_key: authority.vrf.public_key(),
            })
            .collect(),
        QUORUM,
        0,
    )
    .expect("registry");

    db.seed_oracle(&registry, CHAIN_ID).expect("seed");

    Fixture {
        db,
        authorities,
        _dir: dir,
    }
}

impl Fixture {
    fn registry(&self) -> OracleRegistry {
        self.db.oracle_registry().expect("read").expect("seeded")
    }

    fn beacon(&self) -> BeaconState {
        self.db.beacon().expect("read").expect("seeded")
    }

    /// The authority entitled to propose at the current beacon value.
    fn proposer(&self) -> &Authority {
        let registry = self.registry();
        let chosen = registry
            .beacon_proposer(&self.beacon().value)
            .expect("proposer")
            .address;
        self.authorities
            .iter()
            .find(|authority| authority.address == chosen)
            .expect("proposer is in the fixture")
    }

    /// An authority that is *not* entitled to propose right now.
    fn impostor(&self) -> &Authority {
        let chosen = self.proposer().address;
        self.authorities
            .iter()
            .find(|authority| authority.address != chosen)
            .expect("some other authority")
    }

    /// The next nonce `authority` may use.
    ///
    /// Read from committed state rather than tracked by hand. Which authority
    /// is entitled to propose depends on the beacon, so a test that counted
    /// nonces itself would have to predict the selection — and would fail for
    /// that reason rather than for the reason it was written.
    fn nonce_of(&self, authority: &Authority) -> u64 {
        self.db
            .get_account(&authority.address)
            .expect("account")
            .nonce
    }

    /// A beacon submission by `authority` for `height`, valid or not.
    fn beacon_tx(&self, authority: &Authority, height: u64, nonce: u64) -> Transaction {
        let alpha = beacon_alpha(&self.beacon().value, height);
        let proof = prove(&authority.vrf, &alpha).expect("prove");
        signed(
            TxKind::SubmitBeacon(BeaconSubmission {
                height,
                proof: *proof.as_bytes(),
            }),
            nonce,
            &authority.signing,
        )
    }

    /// A feed submission signed by the first `count` authorities, each
    /// attesting `values[i]`.
    fn feed_tx(
        &self,
        feed_id: [u8; 32],
        round: u64,
        observed_height: u64,
        values: &[u64],
        nonce: u64,
    ) -> Transaction {
        let observations = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let authority = &self.authorities[index];
                let message = observation_bytes(&feed_id, round, *value, observed_height);
                FeedObservation {
                    value: *value,
                    public_key: Box::new(authority.signing.public_key()),
                    signature: Box::new(authority.signing.sign(&message).expect("sign")),
                }
            })
            .collect();

        signed(
            TxKind::SubmitFeed(Box::new(FeedSubmission {
                feed_id,
                round,
                observed_height,
                observations,
            })),
            nonce,
            &self.authorities[0].signing,
        )
    }

    fn apply(&self, transactions: Vec<Transaction>, height: u64) -> Result<[u8; 32], NodeError> {
        self.db
            .apply_block(&block_of(transactions), BlockContext::at_height(height))
    }

    /// Creates `MAYA/USD` and returns its identifier.
    fn create_feed(&self, nonce: u64) -> [u8; 32] {
        let name = feed_name("MAYA/USD");
        self.apply(
            vec![signed(
                TxKind::CreateFeed(FeedCreation { name }),
                nonce,
                &self.authorities[0].signing,
            )],
            1,
        )
        .expect("create feed");
        derive_feed_id(&name)
    }
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

#[test]
fn a_quorum_below_a_majority_is_refused() {
    // Two disjoint quorums would each be able to set a different value for one
    // round, both perfectly valid. "The quorum agreed" then means nothing.
    let authorities: Vec<OracleAuthority> = (0..5u8)
        .map(|index| OracleAuthority {
            address: [index; 32],
            vrf_key: VrfSecretKey::from_seed([index + 1; 32]).public_key(),
        })
        .collect();

    assert!(OracleRegistry::new(authorities.clone(), 2, 0).is_err());
    assert!(OracleRegistry::new(authorities.clone(), 3, 0).is_ok());
    // And a quorum larger than the set can never be met.
    assert!(OracleRegistry::new(authorities, 6, 0).is_err());
}

#[test]
fn a_registry_is_ordered_so_it_has_one_encoding() {
    // Two nodes handed the same set in different orders must produce the same
    // state root, or the set itself is a fork.
    let make = |order: &[u8]| {
        OracleRegistry::new(
            order
                .iter()
                .map(|index| OracleAuthority {
                    address: [*index; 32],
                    vrf_key: VrfSecretKey::from_seed([*index + 1; 32]).public_key(),
                })
                .collect(),
            3,
            0,
        )
        .expect("registry")
    };

    assert_eq!(
        make(&[1, 2, 3, 4, 5]).encode(),
        make(&[5, 3, 1, 4, 2]).encode()
    );
}

#[test]
fn a_duplicate_authority_is_refused() {
    // One key repeated is otherwise a quorum on its own.
    let key = VrfSecretKey::from_seed([9; 32]).public_key();
    let authorities = vec![
        OracleAuthority {
            address: [1; 32],
            vrf_key: key,
        },
        OracleAuthority {
            address: [1; 32],
            vrf_key: key,
        },
        OracleAuthority {
            address: [2; 32],
            vrf_key: key,
        },
    ];
    assert!(OracleRegistry::new(authorities, 2, 0).is_err());
}

// ---------------------------------------------------------------------------
// beacon
// ---------------------------------------------------------------------------

#[test]
fn a_valid_proof_from_the_selected_proposer_advances_the_beacon() {
    let fixture = fixture();
    let before = fixture.beacon();

    let tx = fixture.beacon_tx(fixture.proposer(), 1, 0);
    fixture.apply(vec![tx], 1).expect("apply");

    let after = fixture.beacon();
    assert_ne!(after.value, before.value);
    assert_eq!(after.height, 1);
    assert_eq!(after.contributions, 1, "a real VRF output was folded");
}

#[test]
fn a_block_with_no_proof_still_advances_the_beacon_deterministically() {
    // Liveness must not depend on the proposer. If a missing proof stalled the
    // accumulator, one authority going offline would freeze the chain's
    // randomness; if it failed the block, that authority could halt the chain.
    let fixture = fixture();
    let before = fixture.beacon();

    fixture.apply(vec![], 1).expect("an empty block is valid");

    let after = fixture.beacon();
    assert_eq!(after.value, fallback_beacon(&before.value, 1));
    assert_eq!(after.height, 1);
    assert_eq!(
        after.contributions, 0,
        "no VRF output was folded, and the count says so"
    );
}

#[test]
fn the_fallback_is_a_different_value_from_any_real_proof() {
    // The miner's one bit of influence is a choice between these two. They must
    // actually differ, or the "choice" is not one — and they must not collide,
    // or a fallback could be passed off as a proved value.
    let fixture = fixture();
    let before = fixture.beacon();

    let tx = fixture.beacon_tx(fixture.proposer(), 1, 0);
    fixture.apply(vec![tx], 1).expect("apply");

    assert_ne!(fixture.beacon().value, fallback_beacon(&before.value, 1));
}

#[test]
fn a_proof_from_an_authority_that_was_not_selected_is_refused() {
    // With every authority entitled to propose, a miner could choose which of
    // several valid proofs to include and thereby choose among several
    // accumulator values.
    let fixture = fixture();
    let tx = fixture.beacon_tx(fixture.impostor(), 1, 0);

    assert!(matches!(
        fixture.apply(vec![tx], 1).unwrap_err(),
        NodeError::InvalidBeaconProof { .. }
    ));
}

#[test]
fn a_proof_made_for_another_height_is_refused() {
    // Otherwise a proposer could hold a proof back and place it into whichever
    // later block suited them.
    let fixture = fixture();
    let tx = fixture.beacon_tx(fixture.proposer(), 7, 0);

    assert!(matches!(
        fixture.apply(vec![tx], 1).unwrap_err(),
        NodeError::InvalidBeaconProof { .. }
    ));
}

#[test]
fn two_beacon_proofs_in_one_block_are_refused() {
    let fixture = fixture();
    let proposer = fixture.proposer();
    let first = fixture.beacon_tx(proposer, 1, 0);
    let second = fixture.beacon_tx(proposer, 1, 1);

    assert!(matches!(
        fixture.apply(vec![first, second], 1).unwrap_err(),
        NodeError::DuplicateBeaconProof
    ));
}

#[test]
fn the_beacon_is_seeded_from_the_chain_id() {
    // Two networks with identical block histories must not share a randomness
    // sequence, or a lottery on one is a lottery already drawn on the other.
    assert_ne!(
        BeaconState::genesis("maya-mainnet").value,
        BeaconState::genesis("maya-testnet").value
    );
}

#[test]
fn the_accumulator_chains_so_one_output_does_not_determine_it() {
    // Folding rather than replacing is what makes steering block n require
    // steering every block before it.
    let base = BeaconState::genesis(CHAIN_ID);
    let from_a = base.fold(b"output-a", 1).fold(b"output-b", 2);
    let from_b = base.fold(b"output-b", 1).fold(b"output-a", 2);

    assert_ne!(
        from_a.value, from_b.value,
        "the accumulator ignored the order of contributions"
    );
}

// ---------------------------------------------------------------------------
// feeds
// ---------------------------------------------------------------------------

#[test]
fn a_quorum_of_observations_stores_their_median() {
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);

    fixture
        .apply(vec![fixture.feed_tx(feed_id, 1, 2, &[100, 102, 101], 1)], 2)
        .expect("submit");

    let record = fixture.db.feed(&feed_id).expect("read").expect("feed");
    assert_eq!(record.value, 101, "the median, not the first or the mean");
    assert_eq!(record.round, 1);
    assert_eq!(record.observations, 3);
    assert_eq!(
        record.updated_height, 2,
        "the height the chain learned it, not the height the authorities claimed"
    );
}

#[test]
fn one_liar_in_a_quorum_cannot_move_the_median() {
    // The whole security argument for believing a feed. A mean of these is
    // above 3.6 quintillion; the median is 101.
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);

    fixture
        .apply(
            vec![fixture.feed_tx(feed_id, 1, 2, &[100, 101, 102, 103, u64::MAX], 1)],
            2,
        )
        .expect("submit");

    assert_eq!(
        fixture
            .db
            .feed(&feed_id)
            .expect("read")
            .expect("feed")
            .value,
        102
    );
}

#[test]
fn a_submission_below_the_quorum_is_refused() {
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);

    let error = fixture
        .apply(vec![fixture.feed_tx(feed_id, 1, 2, &[100, 101], 1)], 2)
        .unwrap_err();

    assert!(matches!(
        error,
        NodeError::QuorumNotMet {
            supplied: 2,
            required: 3,
            ..
        }
    ));
}

#[test]
fn one_authority_signing_twice_is_not_a_quorum_of_two() {
    // A quorum is a count of *distinct* authorities. Without this check, one
    // key repeated three times clears a threshold of three.
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);

    let authority = &fixture.authorities[0];
    let observation = |value: u64| {
        let message = observation_bytes(&feed_id, 1, value, 2);
        FeedObservation {
            value,
            public_key: Box::new(authority.signing.public_key()),
            signature: Box::new(authority.signing.sign(&message).expect("sign")),
        }
    };

    let tx = signed(
        TxKind::SubmitFeed(Box::new(FeedSubmission {
            feed_id,
            round: 1,
            observed_height: 2,
            observations: vec![observation(100), observation(101), observation(102)],
        })),
        1,
        &authority.signing,
    );

    assert!(matches!(
        fixture.apply(vec![tx], 2).unwrap_err(),
        NodeError::DuplicateObservation { .. }
    ));
}

#[test]
fn a_signature_from_outside_the_registry_is_refused() {
    // Refused rather than ignored: a submitter who could pad a quorum with
    // plausible-looking signatures is the shape most oracle post-mortems take.
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);
    let outsider = generate_signing_key().expect("key");

    let message = observation_bytes(&feed_id, 1, 100, 2);
    let mut observations: Vec<FeedObservation> = fixture
        .authorities
        .iter()
        .take(2)
        .map(|authority| FeedObservation {
            value: 100,
            public_key: Box::new(authority.signing.public_key()),
            signature: Box::new(authority.signing.sign(&message).expect("sign")),
        })
        .collect();

    observations.push(FeedObservation {
        value: 100,
        public_key: Box::new(outsider.public_key()),
        signature: Box::new(outsider.sign(&message).expect("sign")),
    });

    let tx = signed(
        TxKind::SubmitFeed(Box::new(FeedSubmission {
            feed_id,
            round: 1,
            observed_height: 2,
            observations,
        })),
        1,
        &fixture.authorities[0].signing,
    );

    assert!(matches!(
        fixture.apply(vec![tx], 2).unwrap_err(),
        NodeError::NotAnAuthority { .. }
    ));
}

#[test]
fn a_forged_signature_is_refused_even_from_a_real_authority() {
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);

    // A well-formed submission whose values are then altered, leaving the
    // signatures committing to the old ones.
    let mut submission = match fixture.feed_tx(feed_id, 1, 2, &[100, 101, 102], 1).kind {
        TxKind::SubmitFeed(submission) => *submission,
        other => panic!("expected a feed submission, got {}", other.label()),
    };
    submission.observations[0].value = 999;

    let tx = signed(
        TxKind::SubmitFeed(Box::new(submission)),
        1,
        &fixture.authorities[0].signing,
    );
    assert!(fixture.apply(vec![tx], 2).is_err());
}

#[test]
fn replaying_an_old_round_is_refused() {
    // Without this, a quorum's signatures over an old price stay valid forever
    // and can be resubmitted whenever that price suits somebody.
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);

    fixture
        .apply(vec![fixture.feed_tx(feed_id, 5, 2, &[100, 101, 102], 1)], 2)
        .expect("submit");

    let replay = fixture.feed_tx(feed_id, 5, 2, &[100, 101, 102], 2);
    assert!(matches!(
        fixture.apply(vec![replay], 3).unwrap_err(),
        NodeError::StaleFeedRound { .. }
    ));

    let older = fixture.feed_tx(feed_id, 4, 2, &[100, 101, 102], 2);
    assert!(matches!(
        fixture.apply(vec![older], 3).unwrap_err(),
        NodeError::StaleFeedRound { .. }
    ));
}

#[test]
fn a_submission_to_a_feed_nobody_created_is_refused() {
    // Creating one implicitly would let the first quorum to sign a name own it,
    // and a typo'd name would become a second feed that looks like the first.
    let fixture = fixture();
    let phantom = derive_feed_id(&feed_name("BTC/USD"));

    assert!(matches!(
        fixture
            .apply(vec![fixture.feed_tx(phantom, 1, 1, &[100, 101, 102], 0)], 1)
            .unwrap_err(),
        NodeError::UnknownFeed(_)
    ));
}

#[test]
fn only_an_authority_may_create_a_feed() {
    let fixture = fixture();
    let outsider = generate_signing_key().expect("key");
    fixture
        .db
        .put_account(
            &outsider.address(),
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .expect("fund");

    let tx = signed(
        TxKind::CreateFeed(FeedCreation {
            name: feed_name("FAKE/USD"),
        }),
        0,
        &outsider,
    );
    assert!(matches!(
        fixture.apply(vec![tx], 1).unwrap_err(),
        NodeError::NotAnAuthority { .. }
    ));
}

// ---------------------------------------------------------------------------
// rotation
// ---------------------------------------------------------------------------

#[test]
fn the_outgoing_quorum_can_replace_the_authority_set() {
    let fixture = fixture();
    let newcomer = generate_signing_key().expect("key");
    let newcomer_vrf = VrfSecretKey::from_seed([99; 32]);

    let incoming: Vec<(Address, [u8; 33])> = fixture
        .authorities
        .iter()
        .take(2)
        .map(|authority| (authority.address, authority.vrf.public_key().encode()))
        .chain(std::iter::once((
            newcomer.address(),
            newcomer_vrf.public_key().encode(),
        )))
        .collect();

    let message = OracleRegistry::rotation_bytes(1, 2, &incoming);
    let approvals = fixture
        .authorities
        .iter()
        .take(QUORUM as usize)
        .map(|authority| {
            (
                Box::new(authority.signing.public_key()),
                Box::new(authority.signing.sign(&message).expect("sign")),
            )
        })
        .collect();

    let tx = signed(
        TxKind::RotateAuthorities(Box::new(RegistryRotation {
            epoch: 1,
            authorities: incoming,
            quorum: 2,
            approvals,
        })),
        0,
        &fixture.authorities[0].signing,
    );
    fixture.apply(vec![tx], 1).expect("rotate");

    let registry = fixture.registry();
    assert_eq!(registry.epoch, 1);
    assert_eq!(registry.authorities.len(), 3);
    assert_eq!(registry.quorum, 2);
    assert!(registry.position(&newcomer.address()).is_some());
}

#[test]
fn a_rotation_without_a_quorum_is_refused() {
    let fixture = fixture();
    let incoming: Vec<(Address, [u8; 33])> = fixture
        .authorities
        .iter()
        .take(3)
        .map(|authority| (authority.address, authority.vrf.public_key().encode()))
        .collect();

    let message = OracleRegistry::rotation_bytes(1, 2, &incoming);
    let approvals = fixture
        .authorities
        .iter()
        .take(2)
        .map(|authority| {
            (
                Box::new(authority.signing.public_key()),
                Box::new(authority.signing.sign(&message).expect("sign")),
            )
        })
        .collect();

    let tx = signed(
        TxKind::RotateAuthorities(Box::new(RegistryRotation {
            epoch: 1,
            authorities: incoming,
            quorum: 2,
            approvals,
        })),
        0,
        &fixture.authorities[0].signing,
    );
    assert!(matches!(
        fixture.apply(vec![tx], 1).unwrap_err(),
        NodeError::QuorumNotMet { .. }
    ));
}

#[test]
fn a_rotation_to_the_wrong_epoch_is_refused() {
    // The replay guard. Without it, an old approval could reinstate a set that
    // was later rotated away from — which is the attack a rotation mechanism
    // invites if the epoch is left out of what is signed.
    let fixture = fixture();
    let incoming: Vec<(Address, [u8; 33])> = fixture
        .authorities
        .iter()
        .take(3)
        .map(|authority| (authority.address, authority.vrf.public_key().encode()))
        .collect();

    let message = OracleRegistry::rotation_bytes(5, 2, &incoming);
    let approvals = fixture
        .authorities
        .iter()
        .take(QUORUM as usize)
        .map(|authority| {
            (
                Box::new(authority.signing.public_key()),
                Box::new(authority.signing.sign(&message).expect("sign")),
            )
        })
        .collect();

    let tx = signed(
        TxKind::RotateAuthorities(Box::new(RegistryRotation {
            epoch: 5,
            authorities: incoming,
            quorum: 2,
            approvals,
        })),
        0,
        &fixture.authorities[0].signing,
    );
    assert!(matches!(
        fixture.apply(vec![tx], 1).unwrap_err(),
        NodeError::InvalidOracleRegistry { .. }
    ));
}

// ---------------------------------------------------------------------------
// reorgs and the state root
// ---------------------------------------------------------------------------

#[test]
fn reverting_a_block_restores_the_beacon_and_the_feed() {
    // A beacon left at the abandoned chain's value is randomness nobody agreed
    // to; a feed left at the abandoned chain's price is a lie that looks
    // exactly like a fact.
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);
    fixture
        .apply(vec![fixture.feed_tx(feed_id, 1, 2, &[100, 101, 102], 1)], 2)
        .expect("submit");

    let beacon_before = fixture.beacon();
    let feed_before = fixture.db.feed(&feed_id).expect("read").expect("feed");
    let root_before = fixture.db.state_root().expect("root");

    // The proposer is whichever authority the beacon selected, so its nonce is
    // read rather than assumed — and if it happens to be authority zero, the
    // feed transaction above already spent nonce 2.
    let proposer = fixture.proposer();
    let proposer_nonce =
        fixture.nonce_of(proposer) + u64::from(proposer.address == fixture.authorities[0].address);
    let mut block = block_of(vec![
        fixture.feed_tx(feed_id, 2, 3, &[200, 201, 202], 2),
        fixture.beacon_tx(proposer, 3, proposer_nonce),
    ]);
    block.header.state_root = fixture
        .db
        .preview_root(&block, BlockContext::at_height(3))
        .expect("preview");
    let block_id = [42u8; 32];
    fixture
        .db
        .apply_block_journaled(&block, &block_id, BlockContext::at_height(3))
        .expect("apply");
    assert_eq!(
        fixture.db.uncovered_keys().expect("scan"),
        Vec::<Vec<u8>>::new(),
        "every stored key must be under the state root or declared local-only"
    );

    assert_ne!(fixture.beacon().value, beacon_before.value);
    assert_ne!(
        fixture
            .db
            .feed(&feed_id)
            .expect("read")
            .expect("feed")
            .value,
        feed_before.value
    );

    fixture.db.revert_block(&block_id).expect("revert");

    assert_eq!(fixture.beacon(), beacon_before);
    assert_eq!(
        fixture.db.feed(&feed_id).expect("read").expect("feed"),
        feed_before
    );
    assert_eq!(fixture.db.state_root().expect("root"), root_before);
}

#[test]
fn a_chain_with_no_oracle_has_the_state_root_it_always_had() {
    // The oracle folds into the root only when there is something in it, so a
    // network that declines a trusted party does not get one by upgrading.
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    let key = generate_signing_key().expect("key");
    db.put_account(
        &key.address(),
        &Account {
            balance: 1_000,
            nonce: 0,
        },
    )
    .expect("fund");

    let expected = custom_l1_node::state::merkle_root(&[custom_l1_node::state::account_leaf(
        &key.address(),
        &Account {
            balance: 1_000,
            nonce: 0,
        },
    )]);
    assert_eq!(db.state_root().expect("root"), expected);

    // And a block still applies: with no registry, `settle_oracle` does
    // nothing at all rather than seeding one.
    db.apply_block(&block_of(vec![]), BlockContext::at_height(1))
        .expect("apply");
    assert_eq!(db.state_root().expect("root"), expected);
}

#[test]
fn seeding_the_oracle_changes_the_state_root() {
    // The other half: the layer must actually be committed to, or a light
    // client could not verify a price it was handed.
    let fixture = fixture();
    let before = fixture.db.state_root().expect("root");
    fixture.create_feed(0);
    assert_ne!(fixture.db.state_root().expect("root"), before);
}

// ---------------------------------------------------------------------------
// contracts
// ---------------------------------------------------------------------------

/// Reads the beacon and stores it under key `r`.
const RANDOMNESS_WAT: &str = r#"(module
    (import "env" "block_randomness" (func $rand (param i32) (result i32)))
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (func (export "invoke") (param i32) (result i64)
      (local $n i32)
      (i32.store8 (i32.const 0) (i32.const 114))
      (local.set $n (call $rand (i32.const 64)))
      (call $write (i32.const 0) (i32.const 1) (i32.const 64) (local.get $n))
      (i64.const 0)))"#;

/// Reads a feed with a freshness bound and stores the outcome under key `p`.
///
/// The bound is baked in at 5 blocks. The module stores the host function's
/// return code at offset 192 and the price immediately after it at 196, so the
/// twelve bytes it writes are exactly `status ‖ price` with no gap — a test
/// that has to skip padding is a test that will eventually read the padding.
const PRICE_WAT: &str = r#"(module
    (import "env" "oracle_read" (func $read (param i32 i64 i32) (result i32)))
    (import "env" "storage_write" (func $write (param i32 i32 i32 i32)))
    (memory (export "memory") 1)
    (data (i32.const 128) "FEEDID")
    (func (export "invoke") (param i32) (result i64)
      (local $status i32)
      (i32.store8 (i32.const 0) (i32.const 112))
      (local.set $status
        (call $read (i32.const 128) (i64.const 5) (i32.const 196)))
      (i32.store (i32.const 192) (local.get $status))
      (call $write (i32.const 0) (i32.const 1) (i32.const 192) (i32.const 12))
      (i64.const 0)))"#;

fn wasm(text: &str) -> Vec<u8> {
    wat::parse_str(text).expect("valid WAT")
}

/// Deploys `code` and calls it, returning the contract's storage afterwards.
///
/// The nonce comes from committed state rather than from the caller: the tests
/// that reach here have already spent an unpredictable number of the deployer's
/// nonces on beacon proofs, which are signed by whichever authority the beacon
/// selected.
fn run_contract(fixture: &Fixture, code: Vec<u8>, height: u64) -> BTreeMap<Vec<u8>, Vec<u8>> {
    use custom_l1_node::core::payload::{ContractCall, ContractDeploy, derive_contract_id};

    let nonce = fixture.nonce_of(&fixture.authorities[0]);
    let deployer = &fixture.authorities[0].signing;
    let contract = derive_contract_id(&fixture.authorities[0].address, nonce, &code);

    fixture
        .apply(
            vec![
                signed(
                    TxKind::DeployContract(ContractDeploy { code }),
                    nonce,
                    deployer,
                ),
                signed(
                    TxKind::CallContract(ContractCall {
                        contract,
                        input: Vec::new(),
                        gas_limit: 5_000_000,
                    }),
                    nonce + 1,
                    deployer,
                ),
            ],
            height,
        )
        .expect("deploy and call");

    fixture.db.contract_storage(&contract).expect("storage")
}

#[test]
fn a_contract_can_read_the_beacon() {
    let fixture = fixture();
    // Give the beacon a real contribution first, so what the contract reads is
    // a proved value rather than the genesis seed.
    let proposer = fixture.proposer();
    fixture
        .apply(
            vec![fixture.beacon_tx(proposer, 1, fixture.nonce_of(proposer))],
            1,
        )
        .expect("beacon");
    let expected = fixture.beacon().value;

    let storage = run_contract(&fixture, wasm(RANDOMNESS_WAT), 2);
    let stored = storage.get(b"r".as_slice()).expect("randomness written");

    assert_eq!(stored.len(), 32);
    assert_eq!(
        stored.as_slice(),
        expected.as_slice(),
        "the contract saw the previous block's beacon"
    );
}

#[test]
fn a_contract_reading_a_fresh_feed_gets_the_price() {
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);
    fixture
        .apply(vec![fixture.feed_tx(feed_id, 1, 2, &[100, 101, 102], 1)], 2)
        .expect("submit");

    // The module's feed identifier is a data literal, so it has to be patched
    // to the real one before deployment.
    let mut code = PRICE_WAT.to_string();
    code = code.replace(
        "\"FEEDID\"",
        &format!(
            "\"{}\"",
            feed_id.iter().map(escape_byte).collect::<String>()
        ),
    );

    let storage = run_contract(&fixture, wasm(&code), 3);
    let stored = storage.get(b"p".as_slice()).expect("price written");

    let status = i32::from_le_bytes(stored[..4].try_into().expect("status"));
    let value = u64::from_le_bytes(stored[4..12].try_into().expect("value"));
    assert_eq!(status, 8, "the host reported eight bytes written");
    assert_eq!(value, 101, "the median the quorum agreed");
}

#[test]
fn a_contract_reading_a_stale_feed_is_refused_the_price() {
    // The failure mode most oracle post-mortems are made of: a contract that
    // trades on a price nobody has updated in a week. The bound is a required
    // parameter, so forgetting it is not something an author can do.
    let fixture = fixture();
    let feed_id = fixture.create_feed(0);
    fixture
        .apply(vec![fixture.feed_tx(feed_id, 1, 2, &[100, 101, 102], 1)], 2)
        .expect("submit");

    let mut code = PRICE_WAT.to_string();
    code = code.replace(
        "\"FEEDID\"",
        &format!(
            "\"{}\"",
            feed_id.iter().map(escape_byte).collect::<String>()
        ),
    );

    // The module tolerates five blocks; this call is twenty later.
    let storage = run_contract(&fixture, wasm(&code), 22);
    let stored = storage.get(b"p".as_slice()).expect("status written");

    let status = i32::from_le_bytes(stored[..4].try_into().expect("status"));
    let value = u64::from_le_bytes(stored[4..12].try_into().expect("value"));
    assert_eq!(status, -2, "the host refused a stale feed");
    assert_eq!(value, 0, "and wrote no price for the contract to misuse");
}

#[test]

mod common;
fn a_contract_reading_an_unknown_feed_is_told_so() {
    let fixture = fixture();
    let storage = run_contract(&fixture, wasm(PRICE_WAT), 1);
    let stored = storage.get(b"p".as_slice()).expect("status written");

    let status = i32::from_le_bytes(stored[..4].try_into().expect("status"));
    assert_eq!(
        status, -1,
        "an unknown feed is distinguished from a stale one"
    );
}

/// Renders one byte as a WAT string escape.
fn escape_byte(byte: &u8) -> String {
    format!("\\{byte:02x}")
}
