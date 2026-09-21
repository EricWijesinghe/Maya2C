//! Emergency migration of 1,000,000 simulated accounts, with no downtime.
//!
//! **SIM.** Authorization here is a keyed BLAKE3 tag
//! ([`SimAuthorizer`]), not a signature: a million SLH-DSA verifications
//! would measure SLH-DSA, not the migration state machine, which is what this
//! test is for. Real signatures through `EnvelopeAuthorizer` are exercised on
//! the same code paths by `src/agility/tests.rs`.
//!
//! Scenario: SLH-DSA-SHA2-128s is emergency-deprecated at height `START`.
//! 70 % of its accounts rotate to ML-DSA-87 during the window, 20 % register
//! a recovery key and do nothing else, 10 % do nothing. Every block, ordinary
//! transfers keep running between accounts on the default suite, and after
//! sunset a bounded sweep walks the accounts. The test checks, every block,
//! that supply is conserved and that the block's transfers all succeeded —
//! which is what "no downtime" means for a chain.

use maya_crypto_pq::agility::migration::{
    Account, Address, Authorizer, KeyCommitment, MigrationError, MigrationState, claim_message,
    recovery_message, rotate_message, transfer_message,
};
use maya_crypto_pq::agility::{MIN_EMERGENCY_WINDOW, Network, SuitePolicy};
use maya_crypto_pq::suite::SuiteId;

const ACCOUNTS: u32 = 1_000_000;
const ACTIVE_ACCOUNTS: u32 = 1_000;
const START: u64 = 10_000;
const SWEEP_BUDGET: usize = 25_000;
const TRANSFERS_PER_BLOCK: u32 = 200;
const ROTATIONS_PER_BLOCK: u32 = 2_000;
const BALANCE: u128 = 1_000;

/// SIM authorizer: the "signature" is `BLAKE3-keyed(commitment, message)`.
struct SimAuthorizer;

impl Authorizer for SimAuthorizer {
    fn check(&self, commitment: &KeyCommitment, message: &[u8], authorization: &[u8]) -> bool {
        blake3::keyed_hash(commitment, message)
            .as_bytes()
            .as_slice()
            == authorization
    }
}

fn tag(commitment: &KeyCommitment, message: &[u8]) -> Vec<u8> {
    blake3::keyed_hash(commitment, message).as_bytes().to_vec()
}

fn address(i: u32) -> Address {
    *blake3::hash(&i.to_le_bytes()).as_bytes()
}

fn old_key(i: u32) -> KeyCommitment {
    *blake3::hash(&[b"old".as_slice(), &i.to_le_bytes()].concat()).as_bytes()
}

fn new_key(i: u32) -> KeyCommitment {
    *blake3::hash(&[b"new".as_slice(), &i.to_le_bytes()].concat()).as_bytes()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    Rotates,
    CommitsRecovery,
    Absent,
}

fn behaviour(i: u32) -> Behaviour {
    match i % 10 {
        0..=6 => Behaviour::Rotates,
        7 | 8 => Behaviour::CommitsRecovery,
        _ => Behaviour::Absent,
    }
}

fn populate() -> MigrationState {
    let mut state = MigrationState::default();
    for i in 0..ACCOUNTS {
        let account = Account {
            suite: SuiteId::SlhDsaSha2_128s,
            key: old_key(i),
            balance: BALANCE,
            recovery: None,
        };
        state.open(address(i), account).expect("open");
    }
    for j in 0..ACTIVE_ACCOUNTS {
        let i = ACCOUNTS + j;
        let account = Account {
            suite: SuiteId::MlDsa87,
            key: old_key(i),
            balance: BALANCE,
            recovery: None,
        };
        state.open(address(i), account).expect("open");
    }
    state
}

/// Ordinary traffic between the default-suite accounts. Every transfer must
/// succeed, or the chain was down for that block.
fn run_transfers(state: &mut MigrationState, policy: &SuitePolicy, height: u64) {
    let offset = u32::try_from(height % u64::from(ACTIVE_ACCOUNTS)).expect("< ACTIVE_ACCOUNTS");
    for k in 0..TRANSFERS_PER_BLOCK {
        let from_i = ACCOUNTS + (k + offset) % ACTIVE_ACCOUNTS;
        let to_i = ACCOUNTS + (k * 7 + 1 + offset) % ACTIVE_ACCOUNTS;
        let (from, to) = (address(from_i), address(to_i));
        let auth = tag(&old_key(from_i), &transfer_message(&from, &to, 1));
        state
            .transfer(policy, height, (&from, &to), 1, &auth, &SimAuthorizer)
            .unwrap_or_else(|e| panic!("transfer failed at height {height}: {e}"));
    }
}

fn run_window_actions(
    state: &mut MigrationState,
    policy: &SuitePolicy,
    height: u64,
    next: &mut u32,
) {
    let end = (*next + ROTATIONS_PER_BLOCK).min(ACCOUNTS);
    for i in *next..end {
        let (addr, target) = (address(i), (SuiteId::MlDsa87, new_key(i)));
        match behaviour(i) {
            Behaviour::Rotates => {
                let auth = tag(&old_key(i), &rotate_message(&addr, target.0, &target.1));
                state.rotate(policy, height, &addr, target, &auth, &SimAuthorizer)
            }
            Behaviour::CommitsRecovery => {
                let auth = tag(&old_key(i), &recovery_message(&addr, target.0, &target.1));
                state.commit_recovery(policy, height, &addr, target, &auth, &SimAuthorizer)
            }
            Behaviour::Absent => Ok(()),
        }
        .unwrap_or_else(|e| panic!("window action {i} at {height}: {e}"));
    }
    *next = end;
}

#[test]
fn a_million_accounts_migrate_without_downtime() {
    let mut state = populate();
    let supply = state.total_supply();
    let policy = SuitePolicy::genesis(Network::Mainnet)
        .with_deprecation(SuiteId::SlhDsaSha2_128s, START, MIN_EMERGENCY_WINDOW, true)
        .expect("emergency deprecation");
    let sunset = START + MIN_EMERGENCY_WINDOW;

    // The window: actions spread over the first blocks, traffic every block.
    let mut next = 0;
    let mut height = START;
    while next < ACCOUNTS {
        run_window_actions(&mut state, &policy, height, &mut next);
        run_transfers(&mut state, &policy, height);
        assert_eq!(state.total_supply(), supply, "supply at {height}");
        height += 1;
    }
    let window_blocks = height - START;
    assert!(
        height < sunset,
        "the window actions must fit inside the window"
    );

    // Sunset: bounded sweep, traffic continues.
    height = sunset;
    let mut max_examined = 0;
    let mut sweep_blocks = 0;
    loop {
        let report = state.sweep_step(&policy, height, SWEEP_BUDGET);
        max_examined = max_examined.max(report.examined);
        run_transfers(&mut state, &policy, height);
        assert_eq!(state.total_supply(), supply, "supply at {height}");
        sweep_blocks += 1;
        height += 1;
        if report.pass_complete {
            break;
        }
    }

    let (live, vaulted) = state.counts();
    let stragglers = (0..ACCOUNTS)
        .filter(|&i| behaviour(i) != Behaviour::Rotates)
        .count();
    assert_eq!(state.accounts_on(SuiteId::SlhDsaSha2_128s), 0);
    assert_eq!(vaulted, stragglers);
    assert_eq!(live + vaulted, (ACCOUNTS + ACTIVE_ACCOUNTS) as usize);
    assert!(max_examined <= SWEEP_BUDGET);

    // Claims: a registered recovery key opens its entry; no key, no entry.
    let mut claimed = 0;
    let mut locked = 0;
    for i in (0..ACCOUNTS).filter(|&i| behaviour(i) != Behaviour::Rotates) {
        let addr = address(i);
        let auth = tag(&new_key(i), &claim_message(&addr));
        match (
            behaviour(i),
            state.claim_vault(&policy, height, &addr, &auth, &SimAuthorizer),
        ) {
            (Behaviour::CommitsRecovery, Ok(())) => claimed += 1,
            (Behaviour::Absent, Err(MigrationError::Locked)) => locked += 1,
            (b, r) => panic!("account {i}: behaviour {:?}, result {r:?}", b as u8),
        }
    }
    assert_eq!(state.total_supply(), supply);

    println!(
        "migration SIM: {ACCOUNTS} accounts; window actions in {window_blocks} blocks; \
         sweep in {sweep_blocks} blocks (≤{SWEEP_BUDGET} examined/block); \
         {claimed} vault claims, {locked} locked; \
         {TRANSFERS_PER_BLOCK} transfers every block, none failed"
    );
}
