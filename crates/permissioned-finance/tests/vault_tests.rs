//! The CBDC vault end to end: STARK-proved KYC admission, then 50,000
//! compliant settlements with supply conserved throughout.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::collections::BTreeMap;

use maya_permissioned_finance::Account;
use maya_permissioned_finance::cbdc::{Vault, VaultError};
use maya_zk_stark::Proof;
use maya_zk_stark::credential::{
    self, DisclosurePublic, Predicate, credential_leaf, digest_from_bytes, revocation_leaf,
    tree_root,
};
use maya_zk_stark::hash::Digest;

const KYC_SCHEMA: u32 = 3;
const ISSUER: Account = [0xEE; 32];

struct Kyc {
    credentials: Vec<Digest>,
    revocations: Vec<Digest>,
    tiers: Vec<u32>,
}

fn blinding(i: u8) -> Digest {
    digest_from_bytes(&[i ^ 0x5A; 32])
}

impl Kyc {
    /// Account `i` holds credential `i` at `tiers[i]`; `revoked` are revoked.
    fn new(tiers: &[u32], revoked: &[usize]) -> Self {
        Self {
            credentials: tiers
                .iter()
                .enumerate()
                .map(|(i, t)| credential_leaf(&subject(i), KYC_SCHEMA, *t, &blinding(i as u8)))
                .collect(),
            revocations: (0..tiers.len())
                .map(|i| revocation_leaf(revoked.contains(&i)))
                .collect(),
            tiers: tiers.to_vec(),
        }
    }

    fn roots(&self) -> (Digest, Digest) {
        (
            tree_root(&self.credentials).unwrap(),
            tree_root(&self.revocations).unwrap(),
        )
    }

    fn prove(&self, i: usize, tier: u32) -> Result<Proof, maya_zk_stark::ZkError> {
        let (issuer_root, revocation_root) = self.roots();
        let witness = credential::witness_for(
            subject(i),
            KYC_SCHEMA,
            self.tiers[i],
            blinding(i as u8),
            i as u64,
            &self.credentials,
            &self.revocations,
        )?;
        let public = DisclosurePublic {
            issuer_root,
            revocation_root,
            predicate: Predicate::AtLeast(tier),
        };
        credential::prove(&witness, &public)
    }
}

fn subject(i: usize) -> Digest {
    digest_from_bytes(&account(i))
}

fn account(i: usize) -> Account {
    let mut a = [0u8; 32];
    a[0] = i as u8 + 1;
    a
}

fn limits() -> BTreeMap<u32, u64> {
    BTreeMap::from([(1, 1_000), (2, 100_000)])
}

#[test]
fn admission_needs_a_proof_that_meets_the_tier() {
    let kyc = Kyc::new(&[2, 1, 2], &[2]);
    let mut vault = Vault::new(ISSUER, kyc.roots(), limits());

    let proof = kyc.prove(0, 2).unwrap();
    assert_eq!(vault.admit(account(0), 2, &proof), Ok(()));

    // A tier-1 holder cannot even produce a tier-2 proof.
    assert!(
        kyc.prove(1, 2).is_err(),
        "prover refuses an unmet predicate"
    );
    // A tier-2 proof is not a tier-3 admission.
    assert!(matches!(
        vault.admit(account(1), 3, &proof),
        Err(VaultError::BadKycProof(_))
    ));
    // A revoked credential cannot be proved.
    assert!(kyc.prove(2, 1).is_err(), "revoked credential");
    // A proof against another issuer's roots does not verify here.
    let other = Kyc::new(&[2], &[]);
    let mut foreign = Vault::new(ISSUER, other.roots(), limits());
    assert!(matches!(
        foreign.admit(account(0), 2, &proof),
        Err(VaultError::BadKycProof(_))
    ));
}

#[test]
fn fifty_thousand_compliant_settlements_conserve_supply() {
    const ACCOUNTS: usize = 4;
    const SETTLEMENTS: u64 = 50_000;
    let kyc = Kyc::new(&[2, 2, 1, 1], &[]);
    let mut vault = Vault::new(ISSUER, kyc.roots(), limits());
    for i in 0..ACCOUNTS {
        let tier = kyc.tiers[i];
        vault
            .admit(account(i), tier, &kyc.prove(i, tier).unwrap())
            .unwrap();
        vault.issue(ISSUER, account(i), 1_000_000).unwrap();
    }
    let supply = vault.supply();

    let started = std::time::Instant::now();
    for n in 0..SETTLEMENTS {
        let from = (n % ACCOUNTS as u64) as usize;
        let to = (from + 1 + (n / 7 % 3) as usize) % ACCOUNTS;
        // Within every tier's limit, so every one must settle.
        let amount = n % 997 + 1;
        vault.settle(account(from), account(to), amount).unwrap();
    }
    let elapsed = started.elapsed();
    assert_eq!(vault.supply(), supply);
    assert_eq!(vault.held(), u128::from(supply));
    println!(
        "{SETTLEMENTS} settlements in {:.1} ms ({:.0}/s)",
        elapsed.as_secs_f64() * 1e3,
        SETTLEMENTS as f64 / elapsed.as_secs_f64()
    );
}

#[test]
fn limits_freezes_and_the_issuer_are_enforced() {
    let kyc = Kyc::new(&[2, 1], &[]);
    let mut vault = Vault::new(ISSUER, kyc.roots(), limits());
    vault
        .admit(account(0), 2, &kyc.prove(0, 2).unwrap())
        .unwrap();
    vault
        .admit(account(1), 1, &kyc.prove(1, 1).unwrap())
        .unwrap();
    let outsider = [0x77; 32];

    assert_eq!(
        vault.issue(account(0), account(0), 5),
        Err(VaultError::NotIssuer)
    );
    assert_eq!(
        vault.issue(ISSUER, outsider, 5),
        Err(VaultError::NotAdmitted(outsider))
    );
    vault.issue(ISSUER, account(1), 5_000).unwrap();

    assert_eq!(
        vault.settle(account(1), account(0), 1_001),
        Err(VaultError::OverLimit { limit: 1_000 })
    );
    assert_eq!(
        vault.settle(account(1), outsider, 10),
        Err(VaultError::NotAdmitted(outsider))
    );
    vault.set_frozen(ISSUER, account(0), true).unwrap();
    assert_eq!(
        vault.settle(account(1), account(0), 10),
        Err(VaultError::Frozen(account(0)))
    );
    assert_eq!(
        vault.set_frozen(account(1), account(0), false),
        Err(VaultError::NotIssuer)
    );
    vault.set_frozen(ISSUER, account(0), false).unwrap();
    vault.settle(account(1), account(0), 1_000).unwrap();
    assert_eq!(
        vault.settle(account(0), account(1), 1_001),
        Err(VaultError::Insufficient)
    );
    assert_eq!(
        (vault.balance(&account(0)), vault.balance(&account(1))),
        (1_000, 4_000)
    );
}
