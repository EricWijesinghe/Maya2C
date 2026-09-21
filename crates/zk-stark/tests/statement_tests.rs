//! Credentials and sanctions: completeness, native refusals, and a
//! consistent lie per guard (refused by the prover's check in debug and by
//! the verifier in release).

use std::panic::{AssertUnwindSafe, catch_unwind};

use maya_zk_stark::credential::{
    self, DisclosureAir, DisclosurePublic, DisclosureWitness, Predicate, credential_leaf,
    revocation_leaf,
};
use maya_zk_stark::hash::{Digest, F};
use maya_zk_stark::pool::tree::merkle_path;
use maya_zk_stark::sanctions::{self, AbsenceAir, SanctionsList};
use maya_zk_stark::{StarkAir, gadgets::merkle::MerklePath};
use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

fn d(seed: u32) -> Digest {
    core::array::from_fn(|i| F::from_u32(seed * 17 + i as u32 + 1))
}

fn depth16(leaves: &[Digest], index: u64) -> MerklePath {
    let full = merkle_path(leaves, index).expect("path");
    MerklePath {
        siblings: full.siblings[..16].to_vec(),
        index,
    }
}

fn refused<A: StarkAir>(air: &A, trace: RowMajorMatrix<F>, public: &[F]) -> bool {
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        maya_zk_stark::prove(air, trace, public)
    }));
    std::panic::set_hook(quiet);
    match outcome {
        Err(_) | Ok(Err(_)) => true,
        Ok(Ok(proof)) => maya_zk_stark::verify(air, &proof, public).is_err(),
    }
}

/// An issuer with 8 credentials, credential 3 ours (age 34), and a
/// revocation tree where credential 5 is revoked.
fn issuer(age: u32) -> (DisclosureWitness, DisclosurePublic) {
    let (subject, blinding) = (d(1), d(2));
    let leaf = credential_leaf(&subject, 7, age, &blinding);
    let leaves: Vec<Digest> = (0..8)
        .map(|i| if i == 3 { leaf } else { d(100 + i) })
        .collect();
    let revocation: Vec<Digest> = (0..8).map(|i| revocation_leaf(i == 5)).collect();
    let w = DisclosureWitness {
        subject,
        schema: 7,
        value: age,
        blinding,
        credential_path: depth16(&leaves, 3),
        revocation_path: depth16(&revocation, 3),
    };
    let public = DisclosurePublic {
        issuer_root: w.credential_path.root(&leaf),
        revocation_root: w.revocation_path.root(&revocation_leaf(false)),
        predicate: Predicate::AtLeast(18),
    };
    (w, public)
}

fn public_values(p: &DisclosurePublic) -> Vec<F> {
    let (tag, bound) = match p.predicate {
        Predicate::AtLeast(b) => (0, b),
        Predicate::AtMost(b) => (1, b),
        Predicate::EqualTo(b) => (2, b),
    };
    let mut v = p.issuer_root.to_vec();
    v.extend_from_slice(&p.revocation_root);
    v.push(F::from_u32(tag));
    v.push(F::from_u32(bound));
    v
}

#[test]
fn a_live_credential_satisfying_the_predicate_proves() {
    let (w, public) = issuer(34);
    let proof = credential::prove(&w, &public).expect("prove");
    assert_eq!(credential::verify(&proof, &public), Ok(()));
    let mut other = public;
    other.predicate = Predicate::AtLeast(40);
    assert!(
        credential::verify(&proof, &other).is_err(),
        "bound to its predicate"
    );
    for predicate in [Predicate::AtMost(40), Predicate::EqualTo(34)] {
        let p = DisclosurePublic {
            predicate,
            ..public
        };
        let proof = credential::prove(&w, &p).expect("prove");
        assert_eq!(credential::verify(&proof, &p), Ok(()));
    }
}

#[test]
fn natively_refused_disclosures() {
    let (w, public) = issuer(16);
    assert!(credential::prove(&w, &public).is_err(), "predicate fails");
    let (mut w, public) = issuer(34);
    w.revocation_path = depth16(
        &(0..8).map(|i| revocation_leaf(i == 3)).collect::<Vec<_>>(),
        3,
    );
    assert!(credential::prove(&w, &public).is_err(), "revoked");
}

#[test]
fn the_predicate_guard_refuses_an_underage_holder() {
    // Age 16 against AtLeast(18): every hash honest, diff = 16 − 18 wrapped.
    let (w, public) = issuer(16);
    let trace = credential::trace(&w, public.predicate);
    let public = DisclosurePublic {
        issuer_root: w
            .credential_path
            .root(&credential_leaf(&w.subject, 7, 16, &w.blinding)),
        ..public
    };
    assert!(refused(
        &DisclosureAir::default(),
        trace,
        &public_values(&public)
    ));
}

#[test]
fn the_tag_guard_refuses_a_predicate_outside_the_three_shapes() {
    // Tag 3 with value == bound and diff 0 satisfies all three selector
    // branches at once, so only the tag's own range constraint can refuse it.
    let (w, public) = issuer(34);
    let trace = credential::trace(&w, Predicate::EqualTo(34));
    let mut values = public_values(&DisclosurePublic {
        predicate: Predicate::EqualTo(34),
        ..public
    });
    values[2 * maya_zk_stark::hash::DIGEST] = F::from_u32(3);
    assert!(refused(&DisclosureAir::default(), trace, &values));
}

#[test]
fn the_revocation_guard_refuses_a_revoked_credential() {
    // Our credential's slot holds the *revoked* leaf; the revocation root is
    // the real one over that tree.
    let (mut w, public) = issuer(34);
    let revocation: Vec<Digest> = (0..8).map(|i| revocation_leaf(i == 3)).collect();
    w.revocation_path = depth16(&revocation, 3);
    let public = DisclosurePublic {
        revocation_root: w.revocation_path.root(&revocation_leaf(true)),
        ..public
    };
    let trace = credential::trace(&w, public.predicate);
    assert!(refused(
        &DisclosureAir::default(),
        trace,
        &public_values(&public)
    ));
}

#[test]
fn the_index_guard_refuses_an_unrevoked_bit_at_another_position() {
    // Credential 5 is revoked; point the revocation path at position 4
    // (unrevoked) while the credential sits at 5.
    let (subject, blinding) = (d(1), d(2));
    let leaf = credential_leaf(&subject, 7, 34, &blinding);
    let leaves: Vec<Digest> = (0..8)
        .map(|i| if i == 5 { leaf } else { d(100 + i) })
        .collect();
    let revocation: Vec<Digest> = (0..8).map(|i| revocation_leaf(i == 5)).collect();
    let w = DisclosureWitness {
        subject,
        schema: 7,
        value: 34,
        blinding,
        credential_path: depth16(&leaves, 5),
        revocation_path: depth16(&revocation, 4),
    };
    let public = DisclosurePublic {
        issuer_root: w.credential_path.root(&leaf),
        revocation_root: w.revocation_path.root(&revocation_leaf(false)),
        predicate: Predicate::AtLeast(18),
    };
    assert!(credential::prove(&w, &public).is_err(), "native check");
    assert!(refused(
        &DisclosureAir::default(),
        credential::trace(&w, public.predicate),
        &public_values(&public)
    ));
}

// ---------------------------------------------------------------- sanctions

fn id(byte: u8) -> [u8; 32] {
    let mut x = [0x40u8; 32];
    x[31] = byte;
    x
}

#[test]
fn an_unlisted_identifier_clears_and_a_listed_one_cannot_build_a_witness() {
    let list = SanctionsList::build([id(10), id(20), id(30)]).expect("list");
    let root = list.root().expect("root");
    for clear in [id(15), id(5), [0x01; 32], [0xfe; 32]] {
        let w = list.absence_witness(&clear).expect("absent");
        let proof = sanctions::prove(&w, &root).expect("prove");
        assert_eq!(sanctions::verify(&proof, &root), Ok(()));
    }
    assert!(list.absence_witness(&id(20)).is_err());
    assert!(list.contains(&id(20)));
}

#[test]
fn the_gap_guard_refuses_an_identifier_outside_the_bracket() {
    // A listed identifier (20) with the real, adjacent bracket (10, 30) around
    // *another* identifier: the comparison chain cannot hold for 20 < 20.
    let list = SanctionsList::build([id(10), id(20), id(30)]).expect("list");
    let root = list.root().expect("root");
    let mut w = list.absence_witness(&id(15)).expect("absent");
    w.identifier = id(20);
    w.hi = id(20);
    w.hi_path = list.absence_witness(&id(25)).expect("absent").lo_path;
    w.lo = id(10);
    assert!(sanctions::prove(&w, &root).is_err(), "native check");
    let root_values = root.to_vec();
    assert!(refused(
        &AbsenceAir::default(),
        sanctions::trace(&w),
        &root_values
    ));
}

#[test]
fn the_adjacency_guard_refuses_a_non_neighbouring_bracket() {
    // 10 and 30 bracket 20 — and 20 is listed between them. Non-adjacent.
    let list = SanctionsList::build([id(10), id(20), id(30)]).expect("list");
    let root = list.root().expect("root");
    let mut w = list.absence_witness(&id(15)).expect("absent");
    w.identifier = id(20);
    w.hi = id(30);
    w.hi_path = list.absence_witness(&id(35)).expect("absent").lo_path;
    assert!(sanctions::prove(&w, &root).is_err(), "native check");
    assert!(refused(
        &AbsenceAir::default(),
        sanctions::trace(&w),
        &root.to_vec()
    ));
}
