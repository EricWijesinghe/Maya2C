//! Each reference app's user story, end to end, in process.

#![allow(clippy::unwrap_used)]

use maya_clear_sign::{Effect, Warning};
use maya_design_system::guard::Token;
use maya_dex::amm::Pool;
use maya_dex::fees::FeeSchedule;
use maya_dex::types::Direction;
use maya_governance::error::GovernanceError;
use maya_governance::tally::Choice;
use maya_privacy::{ViewingKey, open};
use maya_quantum_harbor::Exposure;
use maya_reference_apps::dao::{self, DaoError, Rules};
use maya_reference_apps::dex_front::{FrontError, quote, review_swap};
use maya_reference_apps::payments::{Checkout, authorise_till, charge};
use maya_reference_apps::vault::{self, VaultRules};
use maya_reference_apps::wallet::{Key, Wallet};
use maya_smart_account::Registry;
use maya_smart_account::account::AccountError;
use maya_treasury::spending::{SpendError, Treasury};

#[test]
fn payments_the_till_charges_only_the_merchant_within_scope() {
    let mut r = Registry::new();
    let mut alice = Wallet::open(&mut r, [1; 32], 0, 10_000);
    let shop = Wallet::open(&mut r, [2; 32], 0, 0);
    let thief = Wallet::open(&mut r, [3; 32], 0, 0);
    let till = Key::from_seed([4; 32]);
    let checkout = Checkout {
        merchant: shop.id,
        max_per_payment: 500,
        expires_at: 1_000,
    };
    authorise_till(&mut r, &mut alice, &till, &checkout, 1_200, 1).unwrap();

    let receipt = charge(&mut r, &mut alice, &till, shop.id, 450, 10).unwrap();
    println!("receipt: {receipt}");
    assert!(
        receipt.contains("450 base units of the native coin"),
        "{receipt}"
    );
    assert_eq!(r.get(&shop.id).unwrap().balance, 450);

    let wrong = charge(&mut r, &mut alice, &till, thief.id, 10, 11);
    assert_eq!(
        wrong,
        Err(AccountError::OutOfScope),
        "a stolen till key cannot pay anyone else"
    );
    let big = charge(&mut r, &mut alice, &till, shop.id, 501, 12);
    assert_eq!(big, Err(AccountError::OutOfScope), "nor overcharge");
    charge(&mut r, &mut alice, &till, shop.id, 500, 13).unwrap();
    let over_day = charge(&mut r, &mut alice, &till, shop.id, 300, 14);
    assert!(
        matches!(over_day, Err(AccountError::Policy(_))),
        "the daily limit still binds: {over_day:?}"
    );
    let late = charge(&mut r, &mut alice, &till, shop.id, 1, 1_001);
    assert_eq!(
        late,
        Err(AccountError::OutOfScope),
        "and the session expires"
    );
    assert_eq!(r.get(&shop.id).unwrap().balance, 950);
}

#[test]
fn dex_front_end_quotes_from_the_curve_refuses_look_alikes_and_catches_a_lying_page() {
    let pool = Pool::empty(FeeSchedule::standard())
        .add_liquidity(1_000_000_000, 2_000_000_000)
        .unwrap()
        .pool;
    let verified = [Token {
        symbol: "MAYA",
        contract: "native",
    }];
    let maya = verified[0];

    let small = quote(
        &pool,
        Direction::BaseToQuote,
        1_000_000,
        50,
        maya,
        &verified,
    )
    .unwrap();
    let large = quote(
        &pool,
        Direction::BaseToQuote,
        100_000_000,
        50,
        maya,
        &verified,
    )
    .unwrap();
    println!("small: {small:?}\nlarge: {large:?}");
    assert!(small.expected_out < 2_000_000, "never the spot price");
    assert!(
        small.min_out < small.expected_out && small.min_out >= small.expected_out * 995 / 1_000
    );
    assert!(!small.high_impact);
    assert!(large.high_impact, "a 10% trade needs a second confirmation");

    let fake = Token {
        symbol: "MАYA",
        contract: "evil",
    };
    assert_eq!(
        quote(&pool, Direction::BaseToQuote, 1_000, 50, fake, &verified),
        Err(FrontError::LookAlike("MAYA".into()))
    );

    let pool_addr = [9u8; 32];
    let honest = [Effect::Transfer {
        token: [0; 32],
        to: pool_addr,
        amount: 1_000_000,
    }];
    let balances = vec![([0u8; 32], 50_000_000u128)];
    assert!(review_swap(&honest, &honest, pool_addr, balances.clone()).is_empty());
    let lying = [
        honest[0].clone(),
        Effect::Approve {
            token: [0; 32],
            spender: [66; 32],
            amount: 1 << 100,
        },
    ];
    let w = review_swap(&honest, &lying, pool_addr, balances);
    assert!(
        w.contains(&Warning::ClaimMismatch) && w.contains(&Warning::UnlimitedApproval),
        "{w:?}"
    );
}

const RULES: Rules = Rules {
    voting_blocks: 480,
    timelock_blocks: 960,
    quorum_bps: 2_000,
    approval_bps: 6_000,
    eligible: 1_000,
};

#[test]
fn dao_a_passed_grant_pays_after_the_timelock_and_the_epoch_limit_binds() {
    let mut t = Treasury::new(100_000, 10_000, 30_000, 1);
    let mut g = dao::propose(&mut t, &RULES, [7; 32], 20_000, "audit grant", 10).unwrap();
    dao::vote(&mut g, Choice::For, 300, 20).unwrap();
    dao::vote(&mut g, Choice::Against, 100, 21).unwrap();
    assert!(matches!(
        dao::execute(&mut t, &mut g, &RULES, 900),
        Err(DaoError::Governance(
            GovernanceError::TimelockNotElapsed { .. }
        ))
    ));
    dao::execute(&mut t, &mut g, &RULES, 1_450).unwrap();
    assert_eq!(t.balance, 80_000);

    let mut second = dao::propose(&mut t, &RULES, [8; 32], 20_000, "second", 1_460).unwrap();
    dao::vote(&mut second, Choice::For, 900, 1_470).unwrap();
    assert_eq!(
        dao::execute(&mut t, &mut second, &RULES, 2_900),
        Err(DaoError::Treasury(SpendError::OverEpochLimit)),
        "a captured vote still cannot exceed the epoch limit"
    );

    let mut failed = dao::propose(&mut t, &RULES, [9; 32], 1, "no quorum", 3_000).unwrap();
    dao::vote(&mut failed, Choice::For, 50, 3_010).unwrap();
    assert_eq!(
        dao::execute(&mut t, &mut failed, &RULES, 4_500),
        Err(DaoError::Rejected)
    );
}

#[test]
fn vault_holds_a_large_withdrawal_guardians_cancel_theft_and_the_auditor_reads_only_memos() {
    let mut r = Registry::new();
    let mut owner = Wallet::open(&mut r, [1; 32], 0, 1_000_000);
    let payee = Wallet::open(&mut r, [2; 32], 0, 0);
    let guardian = Wallet::open(&mut r, [3; 32], 0, 0);
    let rules = VaultRules {
        guardians: vec![guardian.id],
        threshold: 1,
        delay_above: 10_000,
        delay_blocks: 100,
    };
    vault::configure(&mut r, &mut owner, &rules, 1).unwrap();
    let auditor = ViewingKey::generate();

    let env = vault::withdraw(
        &mut r,
        &mut owner,
        payee.id,
        5_000,
        b"payroll 09",
        &auditor.ek,
        10,
    )
    .unwrap();
    assert_eq!(
        r.get(&payee.id).unwrap().balance,
        5_000,
        "small withdrawals are immediate"
    );
    assert_eq!(open(&env, &auditor).unwrap(), b"payroll 09");

    // A thief with the key tries to empty it: held, and a guardian cancels.
    vault::withdraw(&mut r, &mut owner, payee.id, 900_000, b"?", &auditor.ek, 20).unwrap();
    assert_eq!(r.get(&payee.id).unwrap().balance, 5_000, "held, not sent");
    r.guardian_cancel(&owner.id, 0, &[guardian.id]).unwrap();
    assert_eq!(
        r.get(&owner.id).unwrap().balance,
        995_000,
        "the vault is whole"
    );
    assert!(
        open(&env, &ViewingKey::generate()).is_err(),
        "no one else reads the memo"
    );
}

#[test]
fn vault_onboarding_flags_an_exposed_bitcoin_source() {
    let p2pk = [&[0x41][..], &[0x04; 65], &[0xac]].concat();
    let p2wpkh = [&[0x00, 0x14][..], &[0x11; 20]].concat();
    let (e, text) = vault::bitcoin_source_check(&p2pk);
    assert_eq!(e, Exposure::Exposed);
    println!("P2PK source: {text}");
    assert_eq!(
        vault::bitcoin_source_check(&p2wpkh).0,
        Exposure::HashedUntilSpent
    );
}
