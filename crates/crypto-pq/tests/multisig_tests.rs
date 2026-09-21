//! m-of-n ML-DSA policies: a 3-of-5 that signs with two members down, and
//! every way a quorum can be faked.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_crypto_pq::multisig::{Approval, MAX_SIGNERS, MultisigError, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{
    Ed25519, MasterSeed, MlDsa65, MlDsa87, SignatureSuite, SlhDsaShake256f, SuiteId,
};

const MESSAGE: &[u8] = b"release 40 units from the treasury";

/// One member's signing function.
type Signer = Box<dyn Fn(&[u8]) -> Vec<u8>>;

/// Five members: three ML-DSA-87, one ML-DSA-65, one SLH-DSA-SHAKE-256f —
/// a cold-vault member in a different family, so one break is not all five.
struct Committee {
    policy: MultisigPolicy,
    sign: Vec<Signer>,
}

fn member<S: SignatureSuite + 'static>(seed: u8) -> (PolicyKey, Signer)
where
    S::SigningKey: 'static,
{
    let key = S::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]));
    let public = PolicyKey {
        suite: S::ID,
        public_key: S::public_key(&key),
    };
    (
        public,
        Box::new(move |m: &[u8]| S::sign(&key, m).expect("sign")),
    )
}

fn committee(threshold: usize) -> Committee {
    let members = vec![
        member::<MlDsa87>(1),
        member::<MlDsa87>(2),
        member::<MlDsa87>(3),
        member::<MlDsa65>(4),
        member::<SlhDsaShake256f>(5),
    ];
    let (keys, sign): (Vec<_>, Vec<_>) = members.into_iter().unzip();
    Committee {
        policy: MultisigPolicy::new(threshold, keys).expect("policy"),
        sign,
    }
}

fn approve(c: &Committee, indices: &[u8], message: &[u8]) -> Vec<Approval> {
    indices
        .iter()
        .map(|&index| Approval {
            index,
            signature: (c.sign[usize::from(index)])(message),
        })
        .collect()
}

#[test]
fn three_of_five_signs_with_two_members_offline() {
    let c = committee(3);
    // Members 0 and 3 are down; 1, 2 and the SLH-DSA cold member sign.
    let approvals = approve(&c, &[1, 2, 4], MESSAGE);
    assert_eq!(c.policy.verify(MESSAGE, &approvals), Ok(()));
    // Any quorum works, not one blessed subset.
    assert_eq!(
        c.policy.verify(MESSAGE, &approve(&c, &[0, 3, 4], MESSAGE)),
        Ok(())
    );
}

#[test]
fn two_of_three_needed_signatures_is_refused() {
    let c = committee(3);
    let approvals = approve(&c, &[0, 4], MESSAGE);
    assert_eq!(
        c.policy.verify(MESSAGE, &approvals),
        Err(MultisigError::BelowThreshold {
            received: 2,
            threshold: 3
        })
    );
}

#[test]
fn one_signer_cannot_count_twice() {
    let c = committee(3);
    let mut approvals = approve(&c, &[0, 1], MESSAGE);
    approvals.push(approvals[1].clone());
    assert_eq!(
        c.policy.verify(MESSAGE, &approvals),
        Err(MultisigError::BadIndex(1))
    );
}

#[test]
fn approvals_out_of_order_or_outside_the_policy_are_refused() {
    let c = committee(3);
    let reversed = approve(&c, &[2, 1, 0], MESSAGE);
    assert_eq!(
        c.policy.verify(MESSAGE, &reversed),
        Err(MultisigError::BadIndex(1))
    );
    let outside = approve(&c, &[0, 1], MESSAGE)
        .into_iter()
        .chain([Approval {
            index: 5,
            signature: vec![0; 4627],
        }])
        .collect::<Vec<_>>();
    assert_eq!(
        c.policy.verify(MESSAGE, &outside),
        Err(MultisigError::BadIndex(5))
    );
}

#[test]
fn a_signature_over_another_message_or_by_another_member_fails() {
    let c = committee(3);
    let mut approvals = approve(&c, &[0, 1, 2], MESSAGE);
    approvals[2].signature = (c.sign[2])(b"release 4000 units");
    assert!(matches!(
        c.policy.verify(MESSAGE, &approvals),
        Err(MultisigError::Suite { index: 2, .. })
    ));
    // Member 2's slot carrying member 1's valid signature.
    let mut swapped = approve(&c, &[0, 1, 2], MESSAGE);
    swapped[2].signature = swapped[1].signature.clone();
    assert!(matches!(
        c.policy.verify(MESSAGE, &swapped),
        Err(MultisigError::Suite { index: 2, .. })
    ));
}

#[test]
fn a_garbage_extra_approval_is_refused_even_above_threshold() {
    let c = committee(3);
    let mut approvals = approve(&c, &[0, 1, 2], MESSAGE);
    let mut junk = approve(&c, &[3], MESSAGE).remove(0);
    junk.signature[0] ^= 1;
    approvals.push(junk);
    assert!(matches!(
        c.policy.verify(MESSAGE, &approvals),
        Err(MultisigError::Suite { index: 3, .. })
    ));
}

#[test]
fn unmeetable_vacuous_duplicated_or_classical_policies_are_refused() {
    let keys = committee(1).policy.keys().to_vec();
    for threshold in [0, 6] {
        assert!(matches!(
            MultisigPolicy::new(threshold, keys.clone()),
            Err(MultisigError::BadThreshold { .. })
        ));
    }
    let mut twice = keys.clone();
    twice[4] = twice[0].clone();
    assert_eq!(
        MultisigPolicy::new(2, twice),
        Err(MultisigError::DuplicateKey {
            first: 0,
            second: 4
        })
    );
    let (ed, _) = member::<Ed25519>(9);
    assert_eq!(
        MultisigPolicy::new(1, vec![ed]),
        Err(MultisigError::ClassicalSuite(SuiteId::Ed25519))
    );
    let seventeen = (0..=u8::try_from(MAX_SIGNERS).expect("fits"))
        .map(|i| member::<MlDsa65>(i + 10).0)
        .collect();
    assert!(matches!(
        MultisigPolicy::new(1, seventeen),
        Err(MultisigError::BadThreshold { signers: 17, .. })
    ));
}

#[test]
fn the_encoding_round_trips_and_the_digest_names_every_choice() {
    let c = committee(3);
    let encoded = c.policy.encode();
    let (decoded, used) = MultisigPolicy::decode(&encoded).expect("decode");
    assert_eq!((decoded.clone(), used), (c.policy.clone(), encoded.len()));
    assert_eq!(decoded.digest(), c.policy.digest());

    let other_threshold = MultisigPolicy::new(2, c.policy.keys().to_vec()).expect("2-of-5");
    let mut reordered_keys = c.policy.keys().to_vec();
    reordered_keys.swap(0, 1);
    let reordered = MultisigPolicy::new(3, reordered_keys).expect("reordered");
    assert_ne!(other_threshold.digest(), c.policy.digest());
    assert_ne!(reordered.digest(), c.policy.digest());

    // Decode re-runs the construction rules: a hand-made 0-of-5 is refused.
    let mut vacuous = encoded.clone();
    vacuous[0] = 0;
    assert!(MultisigPolicy::decode(&vacuous).is_err());
    assert!(MultisigPolicy::decode(&encoded[..encoded.len() - 1]).is_err());
    let mut huge = encoded;
    huge[1] = 200;
    assert!(matches!(
        MultisigPolicy::decode(&huge),
        Err(MultisigError::BadThreshold { signers: 200, .. })
    ));
}
