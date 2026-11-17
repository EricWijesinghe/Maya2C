//! A share that does not open the vault's commitments is refused on arrival and
//! leaves its custodian's slot free for the genuine share.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_custody_mpc::dkg::{Custodian, Roster, VaultPolicy};
use maya_custody_mpc::seal;
use maya_custody_mpc::session::{SigningSession, VaultDescriptor, respond};
use maya_custody_mpc::vss::ShareBody;

#[test]
fn a_forged_share_is_refused_and_leaves_its_slot_free() {
    let policy = VaultPolicy::new(2, 3).expect("policy");
    let members: Vec<Custodian> = (1..=3)
        .map(|i| Custodian::begin(policy, i).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");
    let dealings: Vec<_> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();
    let held: Vec<_> = members
        .iter()
        .map(|c| c.accept(&roster, &dealings).expect("accept"))
        .collect();

    let vault = VaultDescriptor::establish(&held[..2]).expect("establish");
    let mut session = SigningSession::open(vault, b"a transaction".to_vec());
    let request = session.request();

    // Custodian 2's genuine share relabelled as custodian 1. The dealer check
    // passes, so only the commitment check can refuse it.
    let forged = ShareBody {
        index: 1,
        ..held[1].share.clone()
    };
    let sealed = seal::seal(&forged, &request.encapsulation_key, &request.id(), 1).expect("seal");
    assert!(session.accept_sealed(&sealed).is_err());

    // The genuine custodian 1 still takes slot 1, and the quorum signs.
    for share in &held[..2] {
        let genuine = respond(&request, share).expect("respond");
        session.accept_sealed(&genuine).expect("genuine share");
    }
    session.sign().expect("a quorum of genuine shares signs");
}
