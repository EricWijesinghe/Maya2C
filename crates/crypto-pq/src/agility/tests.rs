use super::audit::{FindingKind, audit};
use super::migration::{
    Account, EnvelopeAuthorizer, MigrationError, MigrationState, claim_message, key_commitment,
    recovery_message, rotate_message, transfer_message,
};
use super::*;
use crate::envelope::SignedEnvelope;
use crate::suite::{MasterSeed, MlDsa65, MlDsa87, SignatureSuite, SlhDsaSha2_128s};
use maya_amount::Amount;

const H: u64 = 1_000;

#[test]
fn genesis_defaults_to_ml_dsa_87() {
    assert_eq!(
        SuitePolicy::genesis(Network::Mainnet).default_suite(),
        SuiteId::MlDsa87
    );
}

#[test]
fn ed25519_is_forbidden_off_devnet_and_flagged_on_it() {
    let mainnet = SuitePolicy::genesis(Network::Mainnet);
    assert_eq!(mainnet.status(SuiteId::Ed25519, H), SuiteStatus::Forbidden);
    assert!(!mainnet.may_sign(SuiteId::Ed25519, H));
    assert_eq!(
        mainnet.with_default(SuiteId::Ed25519),
        Err(PolicyError::DefaultNotPermitted(SuiteId::Ed25519))
    );

    let devnet = SuitePolicy::genesis(Network::Devnet);
    assert!(devnet.may_sign(SuiteId::Ed25519, H));
    let findings = audit(&devnet, H);
    assert!(
        findings.iter().any(|f| f.suite == SuiteId::Ed25519
            && f.kind == FindingKind::BelowSecurityFloor { pq_bits: 0 })
    );
}

#[test]
fn the_audit_is_clean_on_a_fresh_mainnet() {
    assert!(audit(&SuitePolicy::genesis(Network::Mainnet), H).is_empty());
}

#[test]
fn governance_can_change_the_default_among_permitted_suites() {
    let policy = SuitePolicy::genesis(Network::Mainnet)
        .with_default(SuiteId::SlhDsaShake256f)
        .expect("permitted");
    assert_eq!(policy.default_suite(), SuiteId::SlhDsaShake256f);
}

#[test]
fn a_deprecation_runs_its_schedule() {
    let policy = SuitePolicy::genesis(Network::Mainnet)
        .with_deprecation(SuiteId::MlDsa65, H, MIN_MIGRATION_WINDOW, false)
        .expect("deprecate");
    let sunset = H + MIN_MIGRATION_WINDOW;
    assert_eq!(policy.status(SuiteId::MlDsa65, H - 1), SuiteStatus::Active);
    assert_eq!(
        policy.status(SuiteId::MlDsa65, H),
        SuiteStatus::Deprecated {
            sunset_height: sunset
        }
    );
    assert!(policy.may_sign(SuiteId::MlDsa65, sunset - 1));
    assert_eq!(policy.status(SuiteId::MlDsa65, sunset), SuiteStatus::Sunset);
    assert!(!policy.may_sign(SuiteId::MlDsa65, sunset));
    assert!(audit(&policy, H).iter().any(|f| f.kind
        == FindingKind::Deprecated {
            sunset_height: sunset
        }));
}

#[test]
fn deprecation_rules_are_enforced() {
    let policy = SuitePolicy::genesis(Network::Mainnet);
    assert_eq!(
        policy.with_deprecation(SuiteId::MlDsa87, H, MIN_MIGRATION_WINDOW, false),
        Err(PolicyError::DeprecatingDefault(SuiteId::MlDsa87))
    );
    assert!(matches!(
        policy.with_deprecation(SuiteId::MlDsa65, H, MIN_MIGRATION_WINDOW - 1, false),
        Err(PolicyError::WindowTooShort { .. })
    ));
    let emergency = policy
        .with_deprecation(SuiteId::MlDsa65, H, MIN_EMERGENCY_WINDOW, true)
        .expect("emergency window");
    assert_eq!(
        emergency.with_deprecation(SuiteId::MlDsa65, H, MIN_MIGRATION_WINDOW, false),
        Err(PolicyError::AlreadyDeprecated(SuiteId::MlDsa65))
    );
    assert_eq!(
        emergency.with_default(SuiteId::MlDsa65),
        Err(PolicyError::DefaultDeprecated(SuiteId::MlDsa65))
    );
    // The original is untouched: changes return new policies.
    assert_eq!(policy.deprecation(SuiteId::MlDsa65), None);
}

// --------------------------------------------------- migration, real keys

struct Holder<S: SignatureSuite> {
    key: S::SigningKey,
    public: Vec<u8>,
}

impl<S: SignatureSuite> Holder<S> {
    fn new(byte: u8) -> Self {
        let key = S::signing_key_from_seed(&MasterSeed::from_bytes([byte; 32]));
        let public = S::public_key(&key);
        Self { key, public }
    }

    fn commitment(&self) -> (SuiteId, [u8; 32]) {
        (S::ID, key_commitment(S::ID, &self.public))
    }

    fn authorize(&self, msg: &[u8]) -> Vec<u8> {
        let sig = S::sign(&self.key, msg).expect("sign");
        SignedEnvelope::new(S::ID, self.public.clone(), sig)
            .expect("envelope")
            .encode()
    }
}

fn deprecated_65() -> SuitePolicy {
    SuitePolicy::genesis(Network::Mainnet)
        .with_deprecation(SuiteId::MlDsa65, H, MIN_EMERGENCY_WINDOW, true)
        .expect("deprecate")
}

fn state_with(address: [u8; 32], old: &Holder<MlDsa65>, balance: u128) -> MigrationState {
    let mut state = MigrationState::default();
    let (suite, key) = old.commitment();
    state
        .open(
            address,
            Account {
                suite,
                key,
                balance: Amount::from_u128(balance),
                recovery: None,
            },
        )
        .expect("open");
    state
}

#[test]
fn an_account_rotates_during_the_window_with_its_real_old_key() {
    let (policy, addr) = (deprecated_65(), [1u8; 32]);
    let old = Holder::<MlDsa65>::new(1);
    let new = Holder::<MlDsa87>::new(2);
    let mut state = state_with(addr, &old, 500);

    let target = new.commitment();
    let forged = new.authorize(&rotate_message(&addr, target.0, &target.1));
    assert_eq!(
        state.rotate(&policy, H + 1, &addr, target, &forged, &EnvelopeAuthorizer),
        Err(MigrationError::Unauthorized),
        "the new key cannot authorize its own adoption"
    );
    let auth = old.authorize(&rotate_message(&addr, target.0, &target.1));
    state
        .rotate(&policy, H + 1, &addr, target, &auth, &EnvelopeAuthorizer)
        .expect("rotate");
    assert_eq!(
        state.account(&addr).map(|a| a.suite),
        Some(SuiteId::MlDsa87)
    );

    // Sunset changes nothing for it.
    state.sweep_step(&policy, H + MIN_EMERGENCY_WINDOW, 10);
    assert_eq!(state.counts(), (1, 0));
}

#[test]
fn a_straggler_is_vaulted_at_sunset_and_only_the_recovery_key_opens_it() {
    let (policy, addr) = (deprecated_65(), [3u8; 32]);
    let old = Holder::<MlDsa65>::new(3);
    let recovery = Holder::<SlhDsaSha2_128s>::new(4);
    let mut state = state_with(addr, &old, 900);

    let target = recovery.commitment();
    let auth = old.authorize(&recovery_message(&addr, target.0, &target.1));
    state
        .commit_recovery(&policy, H + 5, &addr, target, &auth, &EnvelopeAuthorizer)
        .expect("commit");

    let sunset = H + MIN_EMERGENCY_WINDOW;
    let transfer = old.authorize(&transfer_message(&addr, &addr, Amount::from_u64(1)));
    assert_eq!(
        state.transfer(
            &policy,
            sunset,
            (&addr, &addr),
            Amount::from_u64(1),
            &transfer,
            &EnvelopeAuthorizer
        ),
        Err(MigrationError::SuiteCannotSign(SuiteId::MlDsa65))
    );

    let report = state.sweep_step(&policy, sunset, 10);
    assert_eq!((report.swept, report.pass_complete), (1, true));
    assert_eq!(state.total_supply(), Amount::from_u64(900));

    let by_old = old.authorize(&claim_message(&addr));
    assert_eq!(
        state.claim_vault(&policy, sunset, &addr, &by_old, &EnvelopeAuthorizer),
        Err(MigrationError::Unauthorized),
        "the sunset key must not open the vault"
    );
    let by_recovery = recovery.authorize(&claim_message(&addr));
    state
        .claim_vault(&policy, sunset, &addr, &by_recovery, &EnvelopeAuthorizer)
        .expect("claim");
    let account = state.account(&addr).expect("restored");
    assert_eq!(
        (account.suite, account.balance),
        (SuiteId::SlhDsaSha2_128s, Amount::from_u64(900))
    );
}

#[test]
fn an_account_with_no_recovery_key_stays_locked() {
    let (policy, addr) = (deprecated_65(), [5u8; 32]);
    let old = Holder::<MlDsa65>::new(5);
    let mut state = state_with(addr, &old, 7);
    state.sweep_step(&policy, H + MIN_EMERGENCY_WINDOW, 10);
    let attempt = old.authorize(&claim_message(&addr));
    assert_eq!(
        state.claim_vault(
            &policy,
            H + MIN_EMERGENCY_WINDOW,
            &addr,
            &attempt,
            &EnvelopeAuthorizer
        ),
        Err(MigrationError::Locked)
    );
    assert_eq!(state.total_supply(), Amount::from_u64(7));
}

#[test]
fn rotation_to_a_deprecated_suite_is_refused() {
    let (policy, addr) = (deprecated_65(), [6u8; 32]);
    let old = Holder::<MlDsa65>::new(6);
    let mut state = state_with(addr, &old, 1);
    let target = Holder::<MlDsa65>::new(7).commitment();
    let auth = old.authorize(&rotate_message(&addr, target.0, &target.1));
    assert_eq!(
        state.rotate(&policy, H + 1, &addr, target, &auth, &EnvelopeAuthorizer),
        Err(MigrationError::TargetNotActive(SuiteId::MlDsa65))
    );
}

// G13: whatever the schedule, one lattice and one hash-based suite outlive it.
#[test]
fn a_sunset_that_would_leave_no_hash_based_suite_is_refused() {
    let one = SuitePolicy::genesis(Network::Mainnet)
        .with_deprecation(SuiteId::SlhDsaSha2_128s, H, MIN_MIGRATION_WINDOW, false)
        .expect("the other hash-based suite stays live");
    assert!(
        one.with_deprecation(SuiteId::SlhDsaShake256f, H, MIN_MIGRATION_WINDOW, false)
            .is_err(),
        "both SLH-DSA suites retired: a lattice break would leave nothing to move to"
    );
}

#[test]
fn a_sunset_that_would_leave_no_lattice_suite_is_refused() {
    let hash_default = SuitePolicy::genesis(Network::Mainnet)
        .with_default(SuiteId::SlhDsaShake256f)
        .expect("permitted")
        .with_deprecation(SuiteId::MlDsa65, H, MIN_MIGRATION_WINDOW, false)
        .expect("ML-DSA-87 stays live");
    assert!(
        hash_default
            .with_deprecation(SuiteId::MlDsa87, H, MIN_MIGRATION_WINDOW, false)
            .is_err(),
        "both ML-DSA suites retired"
    );
}

#[test]
fn a_hybrid_does_not_count_toward_either_family() {
    // The hybrid stays live, but it needs its SLH-DSA half to verify, so it is
    // no refuge once hash-based signatures fall: still refused.
    let one = SuitePolicy::genesis(Network::Mainnet)
        .with_deprecation(SuiteId::SlhDsaShake256f, H, MIN_MIGRATION_WINDOW, false)
        .expect("one hash-based suite left");
    assert!(
        one.with_deprecation(SuiteId::SlhDsaSha2_128s, H, MIN_MIGRATION_WINDOW, false)
            .is_err()
    );
    // Retiring the hybrid itself touches no floor.
    assert!(
        one.with_deprecation(
            SuiteId::HybridMlDsa65SlhDsa128s,
            H,
            MIN_MIGRATION_WINDOW,
            false
        )
        .is_ok()
    );
}

// G23: the default is an allow-list of suites at category 3 or above.
#[test]
fn the_default_is_one_of_the_category_3_suites_off_devnet() {
    for network in [Network::Mainnet, Network::Testnet] {
        let policy = SuitePolicy::genesis(network);
        let allowed: Vec<SuiteId> = SuiteId::ALL
            .into_iter()
            .filter(|s| policy.with_default(*s).is_ok())
            .collect();
        assert_eq!(
            allowed,
            [SuiteId::MlDsa65, SuiteId::MlDsa87, SuiteId::SlhDsaShake256f],
            "{network:?}"
        );
    }
    // A category-1 suite may still sign; it just cannot be what every new
    // account is created under.
    let mainnet = SuitePolicy::genesis(Network::Mainnet);
    assert!(mainnet.may_sign(SuiteId::SlhDsaSha2_128s, H));
    assert!(mainnet.may_sign(SuiteId::HybridMlDsa65SlhDsa128s, H));
}

// G13 and a real break: the floor must not stand in the way of the response.
#[test]
fn an_emergency_may_retire_a_broken_lattice_family_but_a_schedule_may_not() {
    let moved = SuitePolicy::genesis(Network::Mainnet)
        .with_default(SuiteId::SlhDsaShake256f)
        .expect("hash-based default");
    let emergency = |p: &SuitePolicy, s| p.with_deprecation(s, H, MIN_EMERGENCY_WINDOW, true);
    let lattice_gone = emergency(&moved, SuiteId::MlDsa65)
        .and_then(|p| emergency(&p, SuiteId::MlDsa87))
        .expect("ML-DSA broken: an emergency retires the whole family");
    assert!(!lattice_gone.may_sign(SuiteId::MlDsa87, H + MIN_EMERGENCY_WINDOW));
    assert!(lattice_gone.may_sign(SuiteId::SlhDsaShake256f, H + MIN_EMERGENCY_WINDOW));

    let ordinary = moved
        .with_deprecation(SuiteId::MlDsa65, H, MIN_MIGRATION_WINDOW, false)
        .expect("ML-DSA-87 stays live");
    assert_eq!(
        ordinary.with_deprecation(SuiteId::MlDsa87, H, MIN_MIGRATION_WINDOW, false),
        Err(PolicyError::LastOfFamily {
            suite: SuiteId::MlDsa87,
            family: SuiteFamily::Lattice
        }),
        "an ordinary schedule never retires the last lattice suite"
    );
}

#[test]
fn the_floor_verdict_does_not_depend_on_the_order_of_entries() {
    // Admission sees entries in the order they were voted; a replaying node
    // sees them in key order. Both must agree, or the schedule is a fork.
    let moved = SuitePolicy::genesis(Network::Mainnet)
        .with_default(SuiteId::SlhDsaShake256f)
        .expect("hash-based default");
    let apply = |entries: &[(SuiteId, bool)]| {
        entries.iter().try_fold(moved.clone(), |p, &(s, e)| {
            let window = if e {
                MIN_EMERGENCY_WINDOW
            } else {
                MIN_MIGRATION_WINDOW
            };
            p.with_deprecation(s, H, window, e)
        })
    };
    let voted = apply(&[(SuiteId::MlDsa87, false), (SuiteId::MlDsa65, true)]);
    let keyed = apply(&[(SuiteId::MlDsa65, true), (SuiteId::MlDsa87, false)]);
    assert!(voted.is_ok());
    assert_eq!(voted, keyed);
}

#[test]
fn an_emergency_never_retires_both_families() {
    // Devnet lets a classical suite be the default, so nothing else keeps a
    // post-quantum family alive: the other-family check is the only guard.
    let classical = SuitePolicy::genesis(Network::Devnet)
        .with_default(SuiteId::Ed25519)
        .expect("devnet default");
    let emergency = |p: &SuitePolicy, s| p.with_deprecation(s, H, MIN_EMERGENCY_WINDOW, true);
    let hash_gone = emergency(&classical, SuiteId::SlhDsaSha2_128s)
        .and_then(|p| emergency(&p, SuiteId::SlhDsaShake256f))
        .expect("lattice suites still live");
    let one_lattice = emergency(&hash_gone, SuiteId::MlDsa65).expect("ML-DSA-87 left");
    assert_eq!(
        emergency(&one_lattice, SuiteId::MlDsa87),
        Err(PolicyError::LastOfFamily {
            suite: SuiteId::MlDsa87,
            family: SuiteFamily::Lattice
        })
    );
}
