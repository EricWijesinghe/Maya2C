//! DIDs end to end: registration, rotation, revocation, and a disclosure proof
//! that reveals nothing.
//!
//! ## The test that matters most
//!
//! `no_committed_record_holds_a_claim_preimage` scans committed state for the
//! bytes of a claim after a full issuance cycle. Every other test here checks
//! that something works; that one checks that the chain has not quietly learned
//! a fact about a person — which is the failure no later fix reaches, because
//! every archive node already has the block.
//!
//! It scans rather than reads the types on purpose. "There is no field for it"
//! is an argument about today's code; a scan is an argument about the bytes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::identity_payload::{
    AnchorAttestation, RegisterDid, RevokeDid, RotateDidKey, SetRevocationBit,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxKind};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use maya_identity::attestation::BITS_PER_PAGE;
use maya_identity::{Did, DidDocument};

use maya_zk_stark::credential::{
    self, DisclosurePublic, Predicate, credential_leaf, digest_from_bytes as field_from_bytes,
    revocation_leaf, tree_root,
};
use maya_zk_stark::hash::{Digest, F};
use maya_zk_stark::pool::tree::{digest_from_bytes, digest_to_bytes};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct Fixture {
    db: StateDB,
    _dir: TempDir,
}

fn fixture(funded: &[(Address, u64)]) -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    for (address, balance) in funded {
        db.put_account(
            address,
            &Account {
                balance: *balance,
                nonce: 0,
            },
        )
        .expect("fund");
    }
    Fixture { db, _dir: dir }
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_789_100_000,
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

/// A distinct 1,984-byte key that is not any signer's.
fn device_key(fill: u8) -> Vec<u8> {
    vec![fill; custom_l1_node::core::identity_payload::HYBRID_PUBLIC_KEY_LEN]
}

fn apply(db: &StateDB, kinds: Vec<(TxKind, u64)>, key: &HybridSigningKey, height: u64) {
    let block = block_of(
        kinds
            .into_iter()
            .map(|(kind, nonce)| signed(kind, nonce, key))
            .collect(),
    );
    db.apply_block(&block, BlockContext::at_height(height))
        .expect("apply");
}

fn try_apply(
    db: &StateDB,
    kind: TxKind,
    nonce: u64,
    key: &HybridSigningKey,
    height: u64,
) -> Result<(), NodeError> {
    db.apply_block(
        &block_of(vec![signed(kind, nonce, key)]),
        BlockContext::at_height(height),
    )
    .map(|_| ())
}

fn register(public_key: Vec<u8>) -> TxKind {
    TxKind::RegisterDid(Box::new(RegisterDid {
        public_key,
        endpoints: vec![(
            "CredentialRepository".to_owned(),
            "https://wallet.example.test".to_owned(),
        )],
    }))
}

fn document(db: &StateDB, did: &Did) -> DidDocument {
    db.stored_did_document(did)
        .expect("read")
        .expect("the subject has a document")
}

// ---------------------------------------------------------------------------
// 1. registration
// ---------------------------------------------------------------------------

#[test]
fn a_registration_creates_the_senders_own_did() {
    // The subject is the sender, never a field — so there is no authorisation
    // check to get wrong.
    let owner = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(owner.address(), 1_000)]);
    apply(&fixture.db, vec![(register(device_key(1)), 0)], &owner, 1);

    let did = Did::from_address(owner.address());
    let document = document(&fixture.db, &did);
    assert_eq!(document.did, did);
    assert_eq!(document.verification.epoch, 0);
    assert_eq!(document.verification.public_key, device_key(1));
    assert_eq!(document.endpoints.len(), 1);
    assert!(!document.is_revoked(1));

    // And the identifier round-trips through its string form, which is what a
    // wallet or a QR code actually carries.
    assert_eq!(Did::parse(&did.to_string()).expect("parse"), did);
    assert!(
        did.to_string().len() < 60,
        "{did} is too long to be an identifier"
    );
}

#[test]
fn registering_twice_is_refused() {
    // A document that could be replaced wholesale would make rotation and
    // revocation pointless: a subject would simply re-register at epoch zero.
    let owner = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(owner.address(), 1_000)]);
    apply(&fixture.db, vec![(register(device_key(1)), 0)], &owner, 1);
    assert!(try_apply(&fixture.db, register(device_key(2)), 1, &owner, 2).is_err());
}

#[test]
fn a_key_of_the_wrong_length_never_becomes_a_document() {
    let owner = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(owner.address(), 1_000)]);
    let short = TxKind::RegisterDid(Box::new(RegisterDid {
        public_key: vec![0; 32],
        endpoints: Vec::new(),
    }));
    assert!(try_apply(&fixture.db, short, 0, &owner, 1).is_err());
}

// ---------------------------------------------------------------------------
// 2. rotation and revocation
// ---------------------------------------------------------------------------

#[test]
fn rotation_is_authorised_by_the_key_it_replaces() {
    // The whole security argument, and it needs no new cryptography: the
    // transaction is signed by the pair whose hash is the sender's address, so
    // a rotation from that address is by construction signed by the current
    // key.
    let owner = generate_signing_key().expect("keygen");
    let stranger = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(owner.address(), 1_000), (stranger.address(), 1_000)]);
    apply(&fixture.db, vec![(register(device_key(1)), 0)], &owner, 1);

    let rotate = || {
        TxKind::RotateDidKey(Box::new(RotateDidKey {
            epoch: 1,
            public_key: device_key(2),
        }))
    };

    // A stranger's rotation touches its own DID, which does not exist — so it
    // is refused, and the subject's document is untouched.
    assert!(try_apply(&fixture.db, rotate(), 0, &stranger, 2).is_err());
    let did = Did::from_address(owner.address());
    assert_eq!(
        document(&fixture.db, &did).verification.public_key,
        device_key(1)
    );

    apply(&fixture.db, vec![(rotate(), 1)], &owner, 3);
    let rotated = document(&fixture.db, &did);
    assert_eq!(rotated.verification.epoch, 1);
    assert_eq!(rotated.verification.public_key, device_key(2));
}

#[test]
fn an_epoch_that_does_not_advance_by_one_is_refused() {
    // A gap is a rotation nobody can point at; standing still would let a
    // replayed rotation reinstall an old key.
    let owner = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(owner.address(), 1_000)]);
    apply(&fixture.db, vec![(register(device_key(1)), 0)], &owner, 1);

    for epoch in [0u32, 2, 7] {
        let kind = TxKind::RotateDidKey(Box::new(RotateDidKey {
            epoch,
            public_key: device_key(3),
        }));
        assert!(
            try_apply(&fixture.db, kind, 1, &owner, 2).is_err(),
            "epoch {epoch} was accepted"
        );
    }
}

#[test]
fn a_revoked_subject_stays_revoked_and_cannot_rotate() {
    let owner = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(owner.address(), 1_000)]);
    apply(
        &fixture.db,
        vec![
            (register(device_key(1)), 0),
            (TxKind::RevokeDid(RevokeDid { reason: 1 }), 1),
        ],
        &owner,
        5,
    );

    let did = Did::from_address(owner.address());
    let revoked = document(&fixture.db, &did);
    assert_eq!(revoked.revoked_at, Some(5));
    assert!(revoked.is_revoked(5));

    // Revocation is a state, not a deletion: a deleted document would be
    // indistinguishable from one that never existed, and the two mean opposite
    // things to a verifier.
    assert!(
        fixture
            .db
            .stored_did_document(&did)
            .expect("read")
            .is_some()
    );

    let rotate = TxKind::RotateDidKey(Box::new(RotateDidKey {
        epoch: 1,
        public_key: device_key(9),
    }));
    assert!(try_apply(&fixture.db, rotate, 2, &owner, 6).is_err());
    assert!(
        try_apply(
            &fixture.db,
            TxKind::RevokeDid(RevokeDid { reason: 2 }),
            3,
            &owner,
            7
        )
        .is_err()
    );
}

// ---------------------------------------------------------------------------
// 3. attestations and the bitmap
// ---------------------------------------------------------------------------

#[test]
fn an_issuer_anchors_a_root_and_flips_a_revocation_bit() {
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 10_000)]);
    let did = Did::from_address(issuer.address());

    apply(
        &fixture.db,
        vec![
            (register(device_key(1)), 0),
            (
                TxKind::AnchorAttestation(Box::new(AnchorAttestation {
                    schema: "age-over-18".to_owned(),
                    root: [0xab; 32],
                    expires_at: 9_000,
                })),
                1,
            ),
            (TxKind::SetRevocationBit(SetRevocationBit { index: 77 }), 2),
        ],
        &issuer,
        10,
    );

    let attestation = fixture
        .db
        .stored_attestation(&did, "age-over-18")
        .expect("read")
        .expect("anchored");
    assert_eq!(attestation.root, [0xab; 32]);
    assert_eq!(attestation.anchored_at, 10);
    assert!(attestation.is_live(10));
    assert!(!attestation.is_live(9_000));

    assert!(fixture.db.is_credential_revoked(&did, 77).expect("read"));
    assert!(!fixture.db.is_credential_revoked(&did, 76).expect("read"));
    // An issuer that has written nothing has revoked nothing.
    let quiet = Did::from_address([0xee; 32]);
    assert!(!fixture.db.is_credential_revoked(&quiet, 0).expect("read"));
}

#[test]
fn revoking_the_same_credential_twice_is_refused() {
    // So a no-op cannot be replayed for its fee side effects alone.
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 10_000)]);
    apply(
        &fixture.db,
        vec![
            (register(device_key(1)), 0),
            (TxKind::SetRevocationBit(SetRevocationBit { index: 5 }), 1),
        ],
        &issuer,
        1,
    );
    let again = TxKind::SetRevocationBit(SetRevocationBit { index: 5 });
    assert!(try_apply(&fixture.db, again, 2, &issuer, 2).is_err());
}

#[test]
fn a_revocation_index_lands_on_the_right_page() {
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 10_000)]);
    let did = Did::from_address(issuer.address());
    let far = BITS_PER_PAGE as u64 * 3 + 11;

    apply(
        &fixture.db,
        vec![
            (register(device_key(1)), 0),
            (TxKind::SetRevocationBit(SetRevocationBit { index: far }), 1),
        ],
        &issuer,
        1,
    );
    assert!(fixture.db.is_credential_revoked(&did, far).expect("read"));
    // The same offset on a different page is untouched.
    assert!(!fixture.db.is_credential_revoked(&did, 11).expect("read"));
}

#[test]
fn an_issuer_with_no_document_cannot_attest() {
    // An attestation from a subject nobody can resolve is one whose signature
    // no verifier can check: storage that proves nothing.
    let stranger = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(stranger.address(), 1_000)]);
    let anchor = TxKind::AnchorAttestation(Box::new(AnchorAttestation {
        schema: "residency".to_owned(),
        root: [1; 32],
        expires_at: 0,
    }));
    assert!(try_apply(&fixture.db, anchor, 0, &stranger, 1).is_err());
}

// ---------------------------------------------------------------------------
// 4. the property that cannot be undone
// ---------------------------------------------------------------------------

#[test]
fn no_committed_record_holds_a_claim_preimage() {
    // A chain is permanent and public. A date of birth written to it once is
    // written forever — no migration, no governance vote and no fork reaches
    // it, because every archive node already has the block.
    //
    // Scanned rather than reasoned about: "there is no field for it" is an
    // argument about today's code, and this is an argument about the bytes.
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 10_000)]);
    let did = Did::from_address(issuer.address());

    // A real claim, committed the way an issuer would.
    let subject = field_from_bytes(&[0x5a; 32]);
    let age: u32 = 34;
    let blinding = [F::new(987_654_321); 8];
    let leaf = credential_leaf(&subject, 7, age, &blinding);
    let leaves: Vec<Digest> = (0..8u32)
        .map(|i| if i == 3 { leaf } else { [F::new(i); 8] })
        .collect();
    let root = tree_root(&leaves).expect("root");
    let root_bytes = digest_to_bytes(&root);

    apply(
        &fixture.db,
        vec![
            (register(device_key(1)), 0),
            (
                TxKind::AnchorAttestation(Box::new(AnchorAttestation {
                    schema: "age-over-18".to_owned(),
                    root: root_bytes,
                    expires_at: 0,
                })),
                1,
            ),
        ],
        &issuer,
        20,
    );

    // The claim value, and the subject identifier, must appear nowhere.
    let attestation = fixture
        .db
        .stored_attestation(&did, "age-over-18")
        .expect("read")
        .expect("anchored")
        .encode();
    let document = document(&fixture.db, &did).encode();

    for record in [attestation, document] {
        assert!(
            !contains(&record, &age.to_le_bytes()),
            "a record carries the claim value"
        );
        assert!(
            !contains(&record, &digest_to_bytes(&subject)),
            "a record carries the subject identifier"
        );
        assert!(
            !contains(&record, &digest_to_bytes(&blinding)),
            "a record carries the blinding factor"
        );
    }
}

/// Whether `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

// ---------------------------------------------------------------------------
// 5. selective disclosure against on-chain roots
// ---------------------------------------------------------------------------

/// An issuer's two trees, and the roots a verifier reads from the chain.
struct Credentials {
    credentials: Vec<Digest>,
    revocations: Vec<Digest>,
}

const BLINDING: Digest = [F::new(11); 8];

impl Credentials {
    fn new(subject: Digest, age: u32, index: usize, revoked: &[usize]) -> Self {
        Self {
            credentials: (0..8u32)
                .map(|i| {
                    if i as usize == index {
                        credential_leaf(&subject, 7, age, &BLINDING)
                    } else {
                        credential_leaf(&[F::new(i); 8], 7, 1, &[F::new(i); 8])
                    }
                })
                .collect(),
            revocations: (0..8)
                .map(|i| revocation_leaf(revoked.contains(&i)))
                .collect(),
        }
    }

    fn roots(&self) -> (Digest, Digest) {
        (
            tree_root(&self.credentials).expect("root"),
            tree_root(&self.revocations).expect("root"),
        )
    }

    fn witness(&self, subject: Digest, age: u32, index: u64) -> credential::DisclosureWitness {
        credential::witness_for(
            subject,
            7,
            age,
            BLINDING,
            index,
            &self.credentials,
            &self.revocations,
        )
        .expect("witness")
    }
}

#[test]
fn a_holder_proves_a_claim_without_revealing_it() {
    let subject = field_from_bytes(&[0x11; 32]);
    let trees = Credentials::new(subject, 34, 3, &[]);
    let (issuer_root, revocation_root) = trees.roots();

    let public = DisclosurePublic {
        issuer_root,
        revocation_root,
        predicate: Predicate::AtLeast(18),
    };
    let witness = trees.witness(subject, 34, 3);

    let proof = credential::prove(&witness, &public).expect("prove");
    assert_eq!(credential::verify(&proof, &public), Ok(()));

    // The verifier's whole input is two roots and a predicate. The age is
    // not among them, and neither is the subject: nothing else is public.
    assert_eq!(public.predicate, Predicate::AtLeast(18));
}

#[test]
fn a_revoked_credential_cannot_be_presented() {
    // The in-circuit revocation bit, end to end. Without it a holder proves the
    // claim in zero knowledge and the verifier then reads a bit from the chain
    // — which links the presentation to a credential index and undoes it.
    let subject = field_from_bytes(&[0x22; 32]);
    let trees = Credentials::new(subject, 34, 3, &[3]);
    let (issuer_root, revocation_root) = trees.roots();

    let public = DisclosurePublic {
        issuer_root,
        revocation_root,
        predicate: Predicate::AtLeast(18),
    };
    let witness = trees.witness(subject, 34, 3);

    assert!(matches!(
        credential::prove(&witness, &public),
        Err(maya_zk_stark::ZkError::Unsatisfied(_))
    ));
}

#[test]
fn a_claim_that_fails_the_predicate_cannot_be_proved() {
    let subject = field_from_bytes(&[0x33; 32]);
    let trees = Credentials::new(subject, 16, 3, &[]);
    let (issuer_root, revocation_root) = trees.roots();

    let public = DisclosurePublic {
        issuer_root,
        revocation_root,
        predicate: Predicate::AtLeast(18),
    };
    let witness = trees.witness(subject, 16, 3);

    assert!(credential::prove(&witness, &public).is_err());
}

#[test]
fn a_verifier_checks_the_root_against_what_the_chain_holds() {
    // The link between the two halves: the root a verifier feeds the circuit is
    // the one consensus accepted, because the issuer's hybrid signature was
    // verified natively when the transaction applied.
    let issuer = generate_signing_key().expect("keygen");
    let fixture = fixture(&[(issuer.address(), 10_000)]);
    let did = Did::from_address(issuer.address());

    let subject = field_from_bytes(&[0x44; 32]);
    let trees = Credentials::new(subject, 40, 2, &[]);
    let (issuer_root, revocation_root) = trees.roots();

    let root_bytes = digest_to_bytes(&issuer_root);

    apply(
        &fixture.db,
        vec![
            (register(device_key(1)), 0),
            (
                TxKind::AnchorAttestation(Box::new(AnchorAttestation {
                    schema: "age-over-18".to_owned(),
                    root: root_bytes,
                    expires_at: 0,
                })),
                1,
            ),
        ],
        &issuer,
        30,
    );

    let anchored = fixture
        .db
        .stored_attestation(&did, "age-over-18")
        .expect("read")
        .expect("anchored");
    assert_eq!(anchored.root, root_bytes);

    let public = DisclosurePublic {
        issuer_root: digest_from_bytes(&anchored.root).expect("canonical"),
        revocation_root,
        predicate: Predicate::AtLeast(21),
    };
    let witness = trees.witness(subject, 40, 2);

    let proof = credential::prove(&witness, &public).expect("prove");
    assert_eq!(credential::verify(&proof, &public), Ok(()));
}
