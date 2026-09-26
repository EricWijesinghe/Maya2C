//! Threshold Raccoon's behaviour: the brief's 3-of-5 over the dealerless
//! keygen, through the failures a real quorum meets.
//!
//! The known-answer test against the authors' reference lives beside the
//! code (`src/threshold/kat.rs`), because it drives the reference's own nonce
//! derivation, which is crate-private. Everything here runs through the
//! public API with this crate's sampler and [`dkg`], so no dealer ever holds
//! the key: quorums sign, fewer than `T` cannot, and a crashed, lying or
//! replaying signer makes a session fail rather than produce a bad signature.

#![cfg(feature = "threshold-lattice")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_custody_mpc::error::CustodyError;
use maya_custody_mpc::threshold::dkg::{Commitment, Dealer, PrivateShare, Reveal};
use maya_custody_mpc::threshold::gauss::PolarSampler;
use maya_custody_mpc::threshold::params::{KEY_LEN, MU_LEN, Q, SID_LEN};
use maya_custody_mpc::threshold::protocol::{
    KeyShare, RoundOne, RoundTwo, Signature, VerifyingKey, combine, keygen_dealer, lagrange,
    sign_1, sign_2, sign_3, verify,
};
use maya_custody_mpc::threshold::{activate, ring};

const OVER_BOUND: CustodyError =
    CustodyError::Lattice("the combined signature exceeds the two-norm bound");

#[test]
fn lagrange_coefficients_match_the_reference() {
    // `_lagrange([0, 2, 4], j)` from thrc_core.py, evaluated by the reference.
    let act = [0usize, 2, 4];
    assert_eq!(
        act.map(|j| lagrange(&act, j)),
        [68_728_072_896_514, 137_456_145_793_023, 343_640_364_482_561]
    );
}

#[test]
fn the_scheme_is_named() {
    assert!(activate().expect("chosen").starts_with("TRaccoon-128"));
}

// ---------------------------------------------------------------------------
// behaviour, with no dealer
// ---------------------------------------------------------------------------

const CEREMONY: &[u8] = b"test vault 7";

fn key(tag: u8, i: usize) -> [u8; KEY_LEN] {
    let mut k = [tag; KEY_LEN];
    k[0] = u8::try_from(i).expect("small");
    k
}

/// Round one of a dealerless ceremony for `parties` parties: every dealer,
/// told every commitment.
fn round_one(tag: u8, threshold: usize, parties: usize) -> (Vec<Dealer>, Vec<Commitment>) {
    let mut noise = PolarSampler;
    let (mut dealers, commitments): (Vec<Dealer>, Vec<Commitment>) = (0..parties)
        .map(|i| {
            Dealer::begin(CEREMONY, i, threshold, parties, &key(tag, i), &mut noise).expect("begin")
        })
        .unzip();
    for dealer in &mut dealers {
        dealer
            .commitments_received(commitments.clone())
            .expect("round one");
    }
    (dealers, commitments)
}

/// A 3-of-5 dealerless ceremony. Returns every party's share and the key,
/// after checking all five parties derived the same key.
fn ceremony() -> (VerifyingKey, Vec<KeyShare>) {
    let (dealers, _) = round_one(0xd1, 3, 5);
    let reveals: Vec<Reveal> = dealers
        .iter()
        .map(|d| d.reveal().expect("reveal"))
        .collect();
    let inbox: Vec<Vec<PrivateShare>> = (0..5)
        .map(|to| {
            dealers
                .iter()
                .map(|d| d.private_share(to).expect("share"))
                .collect()
        })
        .collect();
    let mut shares = Vec::new();
    let mut keys = Vec::new();
    for (dealer, received) in dealers.into_iter().zip(&inbox) {
        let (share, vk) = dealer.finish(&reveals, received).expect("finish");
        shares.push(share);
        keys.push(vk);
    }
    assert!(
        keys.windows(2).all(|w| w[0] == w[1]),
        "every party holds the same key"
    );
    (keys.remove(0), shares)
}

struct Rounds {
    first: Vec<RoundOne>,
    second: Vec<RoundTwo>,
    third: Vec<ring::PolyVec>,
}

/// Runs a full session for `act`, honestly.
fn session(
    vk: &VerifyingKey,
    shares: &mut [KeyShare],
    act: &[usize],
    sid: u8,
    mu: &[u8; MU_LEN],
) -> Result<Rounds, CustodyError> {
    let sid = [sid; SID_LEN];
    let mut noise = PolarSampler;
    let mut sessions = Vec::new();
    let mut first = Vec::new();
    for &j in act {
        let (s, r1) = sign_1(
            vk,
            &mut shares[j],
            &sid,
            act,
            mu,
            &key(sid[0], j),
            &mut noise,
        )?;
        sessions.push(s);
        first.push(r1);
    }
    let second = sessions
        .iter_mut()
        .zip(act)
        .map(|(s, &j)| sign_2(s, &shares[j], &first))
        .collect::<Result<Vec<_>, _>>()?;
    let third = sessions
        .iter_mut()
        .zip(act)
        .map(|(s, &j)| sign_3(s, &shares[j], &second))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Rounds {
        first,
        second,
        third,
    })
}

fn sign(
    vk: &VerifyingKey,
    shares: &mut [KeyShare],
    act: &[usize],
    sid: u8,
    mu: &[u8; MU_LEN],
) -> Result<Signature, CustodyError> {
    let r = session(vk, shares, act, sid, mu)?;
    combine(vk, mu, act, &r.first, &r.second, &r.third)
}

#[test]
fn any_three_of_five_sign_with_two_offline_and_no_dealer() {
    let (vk, mut shares) = ceremony();
    let mu = [0x42; MU_LEN];
    for (sid, act) in [(1u8, [0usize, 2, 4]), (2, [1, 3, 4]), (3, [0, 1, 2])] {
        let signature = sign(&vk, &mut shares, &act, sid, &mu).expect("a quorum signs");
        assert!(verify(&vk, &mu, &signature), "quorum {act:?}");
        assert!(
            !verify(&vk, &[0x43; MU_LEN], &signature),
            "quorum {act:?}, other message"
        );
    }
}

#[test]
fn two_of_a_three_of_five_cannot_sign() {
    // Two Lagrange coefficients reconstruct the wrong secret, so the response
    // is enormous and combine's bound refuses it.
    let (vk, mut shares) = ceremony();
    assert_eq!(
        sign(&vk, &mut shares, &[1, 3], 4, &[7; MU_LEN]).err(),
        Some(OVER_BOUND)
    );
}

#[test]
fn a_signer_that_crashes_after_round_one_aborts_the_session_and_a_retry_signs() {
    let (vk, mut shares) = ceremony();
    let mu = [9; MU_LEN];
    let act = [0usize, 1, 2];
    let sid = [5u8; SID_LEN];
    let mut noise = PolarSampler;
    let mut sessions = Vec::new();
    let mut first = Vec::new();
    for &j in &act {
        let (s, r1) =
            sign_1(&vk, &mut shares[j], &sid, &act, &mu, &key(5, j), &mut noise).expect("round 1");
        sessions.push(s);
        first.push(r1);
    }
    // Party 2 is gone before round two: nobody has its round-one broadcast
    // to pair with, so the survivors cannot even start round two.
    assert_eq!(
        sign_2(&mut sessions[0], &shares[0], &first[..2]).err(),
        Some(CustodyError::Lattice(
            "round two needs round one's broadcast from every signer"
        ))
    );
    // A fresh session with a live quorum signs.
    let signature = sign(&vk, &mut shares, &[0, 1, 3], 6, &mu).expect("retry");
    assert!(verify(&vk, &mu, &signature));
}

#[test]
fn a_signer_that_lies_in_round_two_is_caught_before_anyone_answers() {
    let (vk, mut shares) = ceremony();
    let mu = [3; MU_LEN];
    let act = [0usize, 2, 4];
    let sid = [8u8; SID_LEN];
    let mut noise = PolarSampler;
    let mut sessions = Vec::new();
    let mut first = Vec::new();
    for &j in &act {
        let (s, r1) =
            sign_1(&vk, &mut shares[j], &sid, &act, &mu, &key(8, j), &mut noise).expect("round 1");
        sessions.push(s);
        first.push(r1);
    }
    let mut second: Vec<RoundTwo> = sessions
        .iter_mut()
        .zip(&act)
        .map(|(s, &j)| sign_2(s, &shares[j], &first).expect("round 2"))
        .collect();
    // Party 4 reveals a commitment share other than the one it committed to.
    second[2].w[0][0] = (second[2].w[0][0] + 1) % Q;
    assert_eq!(
        sign_3(&mut sessions[0], &shares[0], &second).err(),
        Some(CustodyError::Lattice(
            "a signer's commitment share does not open"
        ))
    );
}

#[test]
fn a_signer_that_lies_in_round_three_yields_no_valid_signature() {
    // One coefficient of one response, off by one. It looks harmless, but `A`
    // spreads it across every coefficient of `A z`, so the hint reconciling
    // `w` with `A z - 2^nu_t c t` is no longer small: combine's bound refuses
    // it and no signature exists at all.
    let (vk, mut shares) = ceremony();
    let mu = [4; MU_LEN];
    let act = [0usize, 2, 4];
    let mut rounds = session(&vk, &mut shares, &act, 9, &mu).expect("session");
    rounds.third[1][0][0] = (rounds.third[1][0][0] + 1) % Q;
    assert_eq!(
        combine(&vk, &mu, &act, &rounds.first, &rounds.second, &rounds.third).err(),
        Some(OVER_BOUND)
    );
}

#[test]
fn a_share_never_answers_the_same_session_twice() {
    let (vk, mut shares) = ceremony();
    let act = [0usize, 2, 4];
    let sid = [10u8; SID_LEN];
    let mut noise = PolarSampler;
    let mu = [1; MU_LEN];
    sign_1(
        &vk,
        &mut shares[0],
        &sid,
        &act,
        &mu,
        &key(10, 0),
        &mut noise,
    )
    .expect("first");
    assert_eq!(
        sign_1(
            &vk,
            &mut shares[0],
            &sid,
            &act,
            &mu,
            &key(11, 0),
            &mut noise
        )
        .err(),
        Some(CustodyError::Lattice(
            "this share has already signed in that session"
        ))
    );
}

#[test]
fn a_reused_key_still_draws_fresh_randomness_in_a_new_session() {
    // The caller's `key` should be fresh each time. If it is not, the nonce
    // is still bound to the session: the same key in two sessions commits to
    // two different w_j, so no two responses ever share an r_j.
    let (vk, mut shares) = ceremony();
    let act = [0usize, 2, 4];
    let mut noise = PolarSampler;
    let reused = key(0x77, 0);
    let (_, one) = sign_1(
        &vk,
        &mut shares[0],
        &[20; SID_LEN],
        &act,
        &[1; MU_LEN],
        &reused,
        &mut noise,
    )
    .expect("one");
    let (_, two) = sign_1(
        &vk,
        &mut shares[0],
        &[21; SID_LEN],
        &act,
        &[1; MU_LEN],
        &reused,
        &mut noise,
    )
    .expect("two");
    assert_ne!(one.cmt, two.cmt);
}

#[test]
fn malformed_calls_are_refused_rather_than_panicking() {
    let (vk, _) = ceremony();
    // An empty signing set reaches combine's sums only after it is refused.
    assert!(combine(&vk, &[0; MU_LEN], &[], &[], &[], &[]).is_err());
    let mut noise = PolarSampler;
    // Rosters the parameters and the 24-bit headers cannot carry.
    for (t, n) in [(1usize, 5usize), (0, 5), (6, 5), (3, 1025)] {
        assert!(
            keygen_dealer(&[1; KEY_LEN], t, n, &mut noise).is_err(),
            "{t}-of-{n}"
        );
        assert!(
            Dealer::begin(CEREMONY, 0, t, n, &[1; KEY_LEN], &mut noise).is_err(),
            "{t}-of-{n}"
        );
    }
    assert!(
        Dealer::begin(CEREMONY, 5, 3, 5, &[1; KEY_LEN], &mut noise).is_err(),
        "index 5 of 5"
    );
}

#[test]
fn a_dealer_reveals_nothing_before_round_one_ends() {
    let mut noise = PolarSampler;
    let (mut dealer, own) =
        Dealer::begin(CEREMONY, 0, 2, 3, &key(0xd4, 0), &mut noise).expect("begin");
    let early = CustodyError::Lattice("nothing is revealed before every commitment is in");
    assert_eq!(dealer.reveal().err(), Some(early.clone()));
    assert_eq!(dealer.private_share(1).err(), Some(early));
    // A roster that puts someone else's commitment in this party's slot.
    let forged = vec![Commitment([9; 32]), own, own];
    assert!(dealer.commitments_received(forged).is_err());
    dealer
        .commitments_received(vec![own, own, own])
        .expect("round one");
    assert!(dealer.reveal().is_ok());
    assert_eq!(
        dealer.private_share(3).err(),
        Some(CustodyError::Lattice("no such party"))
    );
    assert!(
        dealer.commitments_received(vec![own, own, own]).is_err(),
        "round one ends once"
    );
}

#[test]
fn a_dkg_reveal_that_does_not_open_its_commitment_is_refused() {
    let (dealers, _) = round_one(0xd2, 2, 3);
    let mut reveals: Vec<Reveal> = dealers
        .iter()
        .map(|d| d.reveal().expect("reveal"))
        .collect();
    reveals[1].0[0][0] = (reveals[1].0[0][0] + 1) % Q;
    let received: Vec<PrivateShare> = dealers
        .iter()
        .map(|d| d.private_share(0).expect("share"))
        .collect();
    let first = dealers.into_iter().next().expect("dealer 0");
    assert_eq!(
        first.finish(&reveals, &received).err(),
        Some(CustodyError::Lattice(
            "a party's reveal does not open its commitment"
        ))
    );
}

#[test]
fn a_dealer_who_sends_one_party_a_bad_share_costs_availability_not_security() {
    // Party 1 receives dealer 2's round-two message from a *different*
    // ceremony: a share of the wrong polynomial and the wrong pairwise seed.
    // The DKG cannot see that (it is not verifiable -- ADR-014). Signing can:
    // parties 1 and 2 now disagree on seed[2][1], so the round-three MAC check
    // fails between exactly that pair and the session aborts. The damage is a
    // session that fails and points at who to ask, never a signature that
    // passes.
    let (dealers, commitments) = round_one(0xd3, 3, 5);
    let mut noise = PolarSampler;
    let (mut rogue, rogue_commitment) =
        Dealer::begin(b"another ceremony", 2, 3, 5, &key(0xee, 2), &mut noise).expect("rogue");
    let mut rogue_view = commitments.clone();
    rogue_view[2] = rogue_commitment;
    rogue
        .commitments_received(rogue_view)
        .expect("rogue round one");
    let reveals: Vec<Reveal> = dealers
        .iter()
        .map(|d| d.reveal().expect("reveal"))
        .collect();
    let inbox: Vec<Vec<PrivateShare>> = (0..5)
        .map(|to| {
            dealers
                .iter()
                .enumerate()
                .map(|(from, d)| {
                    if to == 1 && from == 2 {
                        rogue.private_share(1)
                    } else {
                        d.private_share(to)
                    }
                })
                .collect::<Result<_, _>>()
                .expect("shares")
        })
        .collect();
    let mut shares = Vec::new();
    let mut vk = None;
    for (dealer, received) in dealers.into_iter().zip(&inbox) {
        let (share, key) = dealer.finish(&reveals, received).expect("finish");
        shares.push(share);
        vk = Some(key);
    }
    let vk = vk.expect("key");
    let mu = [6; MU_LEN];
    assert_eq!(
        sign(&vk, &mut shares, &[0, 1, 2], 12, &mu).err(),
        Some(CustodyError::Lattice(
            "a signer's view of round one differs from this one"
        ))
    );
    let signature = sign(&vk, &mut shares, &[0, 2, 3], 13, &mu).expect("a quorum without party 1");
    assert!(verify(&vk, &mu, &signature));
}

#[test]
fn a_key_share_never_prints_its_secret() {
    let (_, shares) = ceremony();
    let shown = format!("{:?}", shares[0]);
    assert!(shown.contains("redacted"));
}
