//! A 3-of-5 vault, end to end, and every way the brief asked for it to fail.
//!
//! "Simulated node failures" is four different failures wearing one name, and
//! they are not interchangeable:
//!
//! | Failure | Simulated by | Must produce |
//! |---|---|---|
//! | a custodian is offline at signing | leaving it out of the session | a signature, if `t` remain |
//! | too many are offline | leaving `t-1` in | `ShortOfThreshold`, with both numbers |
//! | a custodian is offline during generation | a missing dealing | `MissingDealer`, naming it |
//! | a custodian lies | a share off its own polynomial | `InconsistentShare`, naming the dealer |
//!
//! The last one is the only one that is an attack rather than an outage, and it
//! is the one a crash-fault-only design would let through.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use curve25519_dalek::scalar::Scalar;
use maya_custody_mpc::dkg::{Custodian, CustodianShare, Dealing, Roster, VaultPolicy};
use maya_custody_mpc::error::CustodyError;
use maya_custody_mpc::hybrid::HYBRID_SIGNATURE_LEN;
use maya_custody_mpc::seal;
use maya_custody_mpc::session::{SigningSession, VaultDescriptor, respond};
use maya_custody_mpc::vss::ShareBody;

/// Runs the whole ceremony and returns what each custodian ends up holding.
fn ceremony(threshold: u8, custodians: u8) -> (Roster, Vec<Custodian>, Vec<CustodianShare>) {
    let policy = VaultPolicy::new(threshold, custodians).expect("policy");
    let members: Vec<Custodian> = (1..=custodians)
        .map(|index| Custodian::begin(policy, index).expect("begin"))
        .collect();

    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");

    let dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();

    let held = members
        .iter()
        .map(|c| c.accept(&roster, &dealings).expect("accept"))
        .collect();

    (roster, members, held)
}

#[test]
fn three_of_five_generates_a_vault_and_signs_for_it() {
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    let mut session = SigningSession::open(vault.clone(), b"pay alice 10".to_vec());
    session.contribute(&held[0]).expect("1");
    session.contribute(&held[2]).expect("3");
    session.contribute(&held[4]).expect("5");

    let signature = session.sign().expect("sign");
    assert_eq!(signature.len(), HYBRID_SIGNATURE_LEN);
    assert_eq!(signature.len(), 11_165);
}

#[test]
fn every_quorum_of_the_same_vault_signs_under_the_same_address() {
    // The property an institution actually depends on: which three custodians
    // happen to be awake does not change where the money is.
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    for a in 0..5 {
        for b in (a + 1)..5 {
            for c in (b + 1)..5 {
                let other = VaultDescriptor::establish(&[
                    held[a].clone(),
                    held[b].clone(),
                    held[c].clone(),
                ])
                .expect("establish");
                assert_eq!(other.address, vault.address, "quorum {a},{b},{c}");
                assert_eq!(other.public_key, vault.public_key);
            }
        }
    }
}

#[test]
fn the_same_message_signs_identically_from_different_quorums() {
    // Deterministic signing is load-bearing on this chain: a transaction id
    // hashes its own signature, so two quorums that produced different bytes
    // would give one transaction two identities.
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    let sign_with = |indices: [usize; 3]| {
        let mut session = SigningSession::open(vault.clone(), b"same message".to_vec());
        for i in indices {
            session.contribute(&held[i]).expect("contribute");
        }
        session.sign().expect("sign")
    };

    assert_eq!(sign_with([0, 1, 2]), sign_with([2, 3, 4]));
}

#[test]
fn two_offline_custodians_are_survivable_and_three_are_not() {
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    // Custodians 4 and 5 are down. Three remain; the vault still works.
    let mut healthy = SigningSession::open(vault.clone(), b"tx".to_vec());
    for share in &held[..3] {
        healthy.contribute(share).expect("contribute");
    }
    assert!(healthy.is_ready());
    healthy
        .sign()
        .expect("a 3-of-5 vault signs with exactly three");

    // Custodians 3, 4 and 5 are down. Two remain.
    let mut starved = SigningSession::open(vault, b"tx".to_vec());
    starved.contribute(&held[0]).expect("contribute");
    starved.contribute(&held[1]).expect("contribute");
    assert!(!starved.is_ready());
    assert_eq!(
        starved.sign().err(),
        Some(CustodyError::ShortOfThreshold {
            received: 2,
            threshold: 3
        }),
        "and the error says how many more custodians to wake"
    );
}

#[test]
fn a_custodian_cannot_fill_a_quorum_by_answering_three_times() {
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    let mut session = SigningSession::open(vault, b"tx".to_vec());
    session.contribute(&held[0]).expect("first");
    assert_eq!(
        session.contribute(&held[0]).err(),
        Some(CustodyError::DuplicateContribution(1))
    );
    assert_eq!(session.contributors(), &[1]);
}

#[test]
fn a_share_from_another_vault_is_refused() {
    let (_, _, first) = ceremony(3, 5);
    let (_, _, second) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&first[..3]).expect("establish");

    let mut session = SigningSession::open(vault, b"tx".to_vec());
    session.contribute(&first[0]).expect("own vault");
    assert_eq!(
        session.contribute(&second[1]).err(),
        Some(CustodyError::WrongVault)
    );
}

#[test]
fn mixing_two_vaults_shares_never_reaches_a_signature() {
    // Belt and braces on the previous test: even if the vault check were
    // removed, the commitment check would refuse the reconstruction rather than
    // signing under a key nobody controls.
    let (_, _, first) = ceremony(2, 3);
    let (_, _, second) = ceremony(2, 3);

    let mixed = vec![first[0].clone(), second[1].clone()];
    assert_eq!(
        VaultDescriptor::establish(&mixed).err(),
        Some(CustodyError::WrongVault)
    );
}

#[test]
fn a_corrupted_share_store_is_caught_before_anything_is_signed() {
    // Custodian 2's safe was restored from a bad backup. The arithmetic cannot
    // tell -- it interpolates whatever it is given -- so the commitment check
    // is the only thing standing between this and a signature under a key that
    // owns nothing.
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    let mut corrupted = held[1].clone();
    corrupted.share = ShareBody {
        index: corrupted.share.index,
        value: corrupted.share.value + Scalar::ONE,
        blind: corrupted.share.blind,
    };

    let mut session = SigningSession::open(vault, b"tx".to_vec());
    session.contribute(&held[0]).expect("1");
    session
        .contribute(&corrupted)
        .expect("2, accepted -- nothing here can tell");
    session.contribute(&held[2]).expect("3");

    assert_eq!(session.sign().err(), Some(CustodyError::WrongSeed));
}

#[test]
fn a_dealer_who_deals_a_share_off_its_own_polynomial_is_named() {
    // The Byzantine case, and the reason the sharing is *verifiable*. Custodian
    // 1 seals custodian 3 a share that is valid-looking, correctly addressed,
    // correctly sealed -- and not on the polynomial it committed to.
    let policy = VaultPolicy::new(3, 5).expect("policy");
    let members: Vec<Custodian> = (1..=5)
        .map(|index| Custodian::begin(policy, index).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");

    let mut dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();

    let victim = 3u8;
    let forged = ShareBody {
        index: victim,
        value: Scalar::from(42u64),
        blind: Scalar::from(43u64),
    };
    let replacement = seal::seal(
        &forged,
        &roster.members[usize::from(victim) - 1].encapsulation_key,
        &roster.id.0,
        1,
    )
    .expect("seal");

    let slot = dealings[0]
        .sealed
        .iter_mut()
        .find(|s| s.recipient == victim)
        .expect("victim's slot");
    *slot = replacement;

    // The victim catches it, names the dealer, and the four honest custodians
    // are unaffected -- they were dealt correctly.
    assert_eq!(
        members[usize::from(victim) - 1]
            .accept(&roster, &dealings)
            .err(),
        Some(CustodyError::InconsistentShare {
            dealer: 1,
            recipient: victim
        })
    );
    for honest in [0usize, 1, 3, 4] {
        members[honest]
            .accept(&roster, &dealings)
            .expect("an honest recipient sees nothing wrong, because nothing was");
    }
}

#[test]
fn a_dealer_missing_from_distribution_stops_the_ceremony() {
    // Not a failure to tolerate. A custodian who accepted four of five dealings
    // would hold a share of a different secret from everyone who accepted all
    // five, and the two would not find out until signing.
    let policy = VaultPolicy::new(3, 5).expect("policy");
    let members: Vec<Custodian> = (1..=5)
        .map(|index| Custodian::begin(policy, index).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");

    let dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();

    assert_eq!(
        members[0].accept(&roster, &dealings[..4]).err(),
        Some(CustodyError::MissingDealer(5))
    );
}

#[test]
fn a_dealer_who_commits_to_a_lower_threshold_is_refused() {
    // A commitment vector of two coefficients in a 3-of-5 vault is a 2-of-5
    // secret smuggled into one member's dealing. Left unchecked, two custodians
    // could later reconstruct that dealer's contribution -- which is not the
    // vault key, but is a rule the vault was not built to.
    let policy = VaultPolicy::new(3, 5).expect("policy");
    let members: Vec<Custodian> = (1..=5)
        .map(|index| Custodian::begin(policy, index).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");

    let mut dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();
    dealings[2].commitments.0.truncate(2);

    assert_eq!(
        members[0].accept(&roster, &dealings).err(),
        Some(CustodyError::MalformedCommitment {
            dealer: 3,
            found: 2,
            expected: 3
        })
    );
}

#[test]
fn a_sealed_share_addressed_elsewhere_does_not_open() {
    let policy = VaultPolicy::new(2, 3).expect("policy");
    let members: Vec<Custodian> = (1..=3)
        .map(|index| Custodian::begin(policy, index).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");

    let mut dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();

    // Leave the routing label saying "for custodian 1" and put the ciphertext
    // that was sealed to custodian 2 inside it. This is the case the transport
    // cannot catch and the envelope must: the blob is well-formed, correctly
    // addressed, the right length, and openable by somebody else.
    let stolen = dealings[0]
        .sealed
        .iter()
        .find(|s| s.recipient == 2)
        .expect("slot")
        .body
        .clone();
    dealings[0]
        .sealed
        .iter_mut()
        .find(|s| s.recipient == 1)
        .expect("slot")
        .body = stolen;

    assert_eq!(
        members[0].accept(&roster, &dealings).err(),
        Some(CustodyError::SealFailed(1))
    );
}

#[test]
fn an_incomplete_roster_is_not_a_vault() {
    let policy = VaultPolicy::new(3, 5).expect("policy");
    let members: Vec<Custodian> = (1..=4)
        .map(|index| Custodian::begin(policy, index).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();

    assert_eq!(
        Roster::assemble(policy, &announcements).err(),
        Some(CustodyError::MissingDealer(5))
    );
}

#[test]
fn two_custodians_claiming_one_index_are_refused() {
    let policy = VaultPolicy::new(2, 3).expect("policy");
    let a = Custodian::begin(policy, 2).expect("begin");
    let b = Custodian::begin(policy, 2).expect("begin");
    let c = Custodian::begin(policy, 1).expect("begin");

    assert_eq!(
        Roster::assemble(policy, &[a.announce(), b.announce(), c.announce()]).err(),
        Some(CustodyError::DuplicateContribution(2))
    );
}

#[test]
fn substituting_a_custodian_produces_a_different_vault_id() {
    // The id covers every encapsulation key, so a swapped member is a different
    // vault rather than the same vault with a new member -- and every message
    // between the two rosters is then rejected on arrival.
    let policy = VaultPolicy::new(2, 3).expect("policy");
    let members: Vec<Custodian> = (1..=3)
        .map(|i| Custodian::begin(policy, i).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let original = Roster::assemble(policy, &announcements).expect("roster");

    let mut swapped = announcements.clone();
    swapped[1] = Custodian::begin(policy, 2).expect("begin").announce();
    let other = Roster::assemble(policy, &swapped).expect("roster");

    assert_ne!(original.id, other.id);
}

#[test]
fn a_sealed_response_belongs_to_exactly_one_signing_session() {
    // The network path. A custodian's response is sealed to the combiner's
    // ephemeral key and bound to the session id, so a response captured off the
    // wire cannot be replayed into a later session for a different message.
    let (_, _, held) = ceremony(2, 3);
    let vault = VaultDescriptor::establish(&held[..2]).expect("establish");

    let mut first = SigningSession::open(vault.clone(), b"pay alice 10".to_vec());
    let mut second = SigningSession::open(vault, b"pay mallory 10000".to_vec());
    assert_ne!(first.id(), second.id());

    let response = respond(&first.request(), &held[0]).expect("respond");
    first.accept_sealed(&response).expect("its own session");
    assert_eq!(
        second.accept_sealed(&response).err(),
        Some(CustodyError::SealFailed(1)),
        "a captured response does not carry into another session"
    );
}

#[test]
fn the_network_path_and_the_local_path_produce_the_same_signature() {
    let (_, _, held) = ceremony(3, 5);
    let vault = VaultDescriptor::establish(&held[..3]).expect("establish");

    let mut local = SigningSession::open(vault.clone(), b"tx".to_vec());
    for share in &held[..3] {
        local.contribute(share).expect("contribute");
    }

    let mut remote = SigningSession::open(vault, b"tx".to_vec());
    let request = remote.request();
    for share in &held[..3] {
        let sealed = respond(&request, share).expect("respond");
        remote.accept_sealed(&sealed).expect("accept");
    }

    assert_eq!(local.sign().expect("local"), remote.sign().expect("remote"));
}

#[test]
fn a_custodian_refuses_to_respond_to_another_vaults_request() {
    let (_, _, first) = ceremony(2, 3);
    let (_, _, second) = ceremony(2, 3);
    let vault = VaultDescriptor::establish(&first[..2]).expect("establish");
    let session = SigningSession::open(vault, b"tx".to_vec());

    assert_eq!(
        respond(&session.request(), &second[0]).err(),
        Some(CustodyError::WrongVault)
    );
}

#[test]
fn a_five_of_five_vault_tolerates_nothing_and_says_so() {
    let (_, _, held) = ceremony(5, 5);
    let vault = VaultDescriptor::establish(&held).expect("establish");

    let mut session = SigningSession::open(vault, b"tx".to_vec());
    for share in &held[..4] {
        session.contribute(share).expect("contribute");
    }
    assert_eq!(
        session.sign().err(),
        Some(CustodyError::ShortOfThreshold {
            received: 4,
            threshold: 5
        })
    );
}

#[test]
fn a_policy_that_can_never_be_met_is_refused_at_construction() {
    assert_eq!(VaultPolicy::new(1, 0).err(), Some(CustodyError::EmptyVault));
    assert_eq!(
        VaultPolicy::new(4, 3).err(),
        Some(CustodyError::InvalidThreshold {
            threshold: 4,
            custodians: 3
        })
    );
    assert_eq!(
        VaultPolicy::new(0, 3).err(),
        Some(CustodyError::InvalidThreshold {
            threshold: 0,
            custodians: 3
        })
    );
}

#[test]
fn a_custodian_index_outside_the_roster_is_refused_at_the_start() {
    let policy = VaultPolicy::new(2, 3).expect("policy");
    assert_eq!(
        Custodian::begin(policy, 0).err(),
        Some(CustodyError::ReservedIndex)
    );
    assert_eq!(
        Custodian::begin(policy, 4).err(),
        Some(CustodyError::UnknownCustodian {
            index: 4,
            custodians: 3
        })
    );
}
