//! Smart accounts with real ML-DSA-65 keys (Master Prompts 13 §2, 22).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation
)]

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SlhDsaShake256f, SuiteId};
use maya_smart_account::account::{AccountError, MAX_SIGNATURES};
use maya_smart_account::fees::{FeeSpec, Paymaster, TokenPrice};
use maya_smart_account::op::{Action, Op};
use maya_smart_account::policy::{Policy, SessionScope};
use maya_smart_account::recovery::GuardianSet;
use maya_smart_account::{AccountId, Registry, key_hash};

type Sk = <MlDsa65 as SignatureSuite>::SigningKey;

fn key(b: u8) -> (Sk, Vec<u8>) {
    let sk = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([b; 32]));
    let pk = MlDsa65::public_key(&sk);
    (sk, pk)
}

fn op(account: AccountId, nonce: u64, action: Action) -> Op {
    Op {
        account,
        nonce,
        action,
        fee: FeeSpec::Native { max_fee: 1_000 },
        signatures: vec![],
    }
}

fn signed(mut o: Op, keys: &[(&Sk, &[u8])]) -> Op {
    let msg = o.signing_bytes();
    o.signatures = keys
        .iter()
        .map(|(sk, pk)| {
            (
                key_hash(SuiteId::MlDsa65, pk),
                MlDsa65::sign(sk, &msg).unwrap(),
            )
        })
        .collect();
    o
}

fn setup() -> (Registry, AccountId, (Sk, Vec<u8>), AccountId) {
    let mut r = Registry::new();
    let alice = key(1);
    let a = r.create(SuiteId::MlDsa65, &alice.1, 0, 1_000_000);
    let bob = key(2);
    let b = r.create(SuiteId::MlDsa65, &bob.1, 0, 0);
    (r, a, alice, b)
}

#[test]
fn a_transfer_carries_a_key_hash_not_a_public_key() {
    let (mut r, a, (sk, pk), b) = setup();
    let o = signed(
        op(a, 0, Action::Transfer { to: b, amount: 10 }),
        &[(&sk, &pk)],
    );
    let with_key = o.encoded_len() + pk.len();
    println!(
        "ML-DSA-65 transfer: {} bytes on the wire; {} if it carried the public key",
        o.encoded_len(),
        with_key
    );
    assert!(o.encoded_len() < 3_309 + 200);
    assert_eq!(with_key - o.encoded_len(), 1_952);
    r.apply(&o, 1, 100).unwrap();
    assert_eq!(r.get(&b).unwrap().balance, 10);
}

#[test]
fn rotation_keeps_the_address_rejects_the_old_key_and_accepts_the_new_one() {
    let (mut r, a, (old_sk, old_pk), b) = setup();
    let (new_sk, new_pk) = key(9);
    let rotate = Action::RotateKey {
        old: key_hash(SuiteId::MlDsa65, &old_pk),
        suite: SuiteId::MlDsa65,
        public_key: new_pk.clone(),
    };
    r.apply(&signed(op(a, 0, rotate), &[(&old_sk, &old_pk)]), 1, 10)
        .unwrap();
    let stale = signed(
        op(a, 1, Action::Transfer { to: b, amount: 5 }),
        &[(&old_sk, &old_pk)],
    );
    assert_eq!(r.apply(&stale, 2, 10), Err(AccountError::UnknownKey));
    let fresh = signed(
        op(a, 1, Action::Transfer { to: b, amount: 5 }),
        &[(&new_sk, &new_pk)],
    );
    r.apply(&fresh, 2, 10).unwrap();
    assert_eq!(r.get(&a).unwrap().id, a, "the address did not change");
    assert_eq!(r.get(&b).unwrap().balance, 5);
}

#[test]
fn multi_key_accounts_need_their_threshold() {
    let (mut r, a, (sk1, pk1), b) = setup();
    let (sk2, pk2) = key(3);
    r.apply(
        &signed(
            op(
                a,
                0,
                Action::AddKey {
                    suite: SuiteId::MlDsa65,
                    public_key: pk2.clone(),
                    weight: 1,
                },
            ),
            &[(&sk1, &pk1)],
        ),
        1,
        10,
    )
    .unwrap();
    r.set_threshold(&a, 2);
    let one = signed(
        op(a, 1, Action::Transfer { to: b, amount: 1 }),
        &[(&sk1, &pk1)],
    );
    assert_eq!(r.apply(&one, 2, 10), Err(AccountError::BelowThreshold));
    let two = signed(
        op(a, 1, Action::Transfer { to: b, amount: 1 }),
        &[(&sk1, &pk1), (&sk2, &pk2)],
    );
    r.apply(&two, 2, 10).unwrap();
}

#[test]
fn validation_cost_is_bounded_before_any_signature_is_verified() {
    let (mut r, a, _, b) = setup();
    let mut o = op(a, 0, Action::Transfer { to: b, amount: 1 });
    o.signatures = (0..=MAX_SIGNATURES as u8)
        .map(|i| ([i; 32], vec![0u8; 3_309]))
        .collect();
    assert_eq!(r.apply(&o, 1, 10), Err(AccountError::ValidationBudget));
    // Three SLH-DSA-256f keys cost more than the budget even though the
    // count is fine: refused before the (slow) verification runs.
    let (osk, opk) = key(1);
    let mut nonce = 0;
    let mut hashes = vec![];
    for s in 20..23u8 {
        let sk = SlhDsaShake256f::signing_key_from_seed(&MasterSeed::from_bytes([s; 32]));
        let pk = SlhDsaShake256f::public_key(&sk);
        hashes.push(key_hash(SuiteId::SlhDsaShake256f, &pk));
        r.apply(
            &signed(
                op(
                    a,
                    nonce,
                    Action::AddKey {
                        suite: SuiteId::SlhDsaShake256f,
                        public_key: pk,
                        weight: 1,
                    },
                ),
                &[(&osk, &opk)],
            ),
            1,
            0,
        )
        .unwrap();
        nonce += 1;
    }
    let mut heavy = op(a, nonce, Action::Transfer { to: b, amount: 1 });
    heavy.signatures = hashes.into_iter().map(|h| (h, vec![0u8; 49_856])).collect();
    assert_eq!(r.apply(&heavy, 1, 0), Err(AccountError::ValidationBudget));
}

#[test]
fn policies_limit_spending_and_recipients() {
    let (mut r, a, (sk, pk), b) = setup();
    let (_, cpk) = key(4);
    let c = r.create(SuiteId::MlDsa65, &cpk, 0, 0);
    let policy = Policy {
        daily_limit: 1_000,
        per_recipient_daily: 600,
        allow_list: vec![b],
        ..Policy::default()
    };
    r.apply(
        &signed(op(a, 0, Action::SetPolicy(policy)), &[(&sk, &pk)]),
        1,
        0,
    )
    .unwrap();
    let to_c = signed(
        op(a, 1, Action::Transfer { to: c, amount: 1 }),
        &[(&sk, &pk)],
    );
    assert!(matches!(r.apply(&to_c, 2, 0), Err(AccountError::Policy(_))));
    r.apply(
        &signed(
            op(a, 1, Action::Transfer { to: b, amount: 600 }),
            &[(&sk, &pk)],
        ),
        2,
        0,
    )
    .unwrap();
    let over = signed(
        op(a, 2, Action::Transfer { to: b, amount: 1 }),
        &[(&sk, &pk)],
    );
    assert_eq!(
        r.apply(&over, 3, 0),
        Err(AccountError::Policy("per-recipient daily limit"))
    );
    // Next day the limits reset.
    r.apply(
        &signed(
            op(a, 2, Action::Transfer { to: b, amount: 600 }),
            &[(&sk, &pk)],
        ),
        43_200 + 1,
        0,
    )
    .unwrap();
}

#[test]
fn session_keys_are_scoped_and_expire() {
    let (mut r, a, (sk, pk), b) = setup();
    let (ssk, spk) = key(5);
    let scope = SessionScope {
        recipients: vec![b],
        max_amount: 50,
        expires_at: 100,
    };
    r.apply(
        &signed(
            op(
                a,
                0,
                Action::AddSessionKey {
                    suite: SuiteId::MlDsa65,
                    public_key: spk.clone(),
                    scope,
                },
            ),
            &[(&sk, &pk)],
        ),
        1,
        0,
    )
    .unwrap();
    r.apply(
        &signed(
            op(a, 1, Action::Transfer { to: b, amount: 50 }),
            &[(&ssk, &spk)],
        ),
        10,
        0,
    )
    .unwrap();
    let too_much = signed(
        op(a, 2, Action::Transfer { to: b, amount: 51 }),
        &[(&ssk, &spk)],
    );
    assert_eq!(r.apply(&too_much, 10, 0), Err(AccountError::OutOfScope));
    let admin = signed(
        op(a, 2, Action::SetPolicy(Policy::default())),
        &[(&ssk, &spk)],
    );
    assert_eq!(r.apply(&admin, 10, 0), Err(AccountError::OutOfScope));
    let late = signed(
        op(a, 2, Action::Transfer { to: b, amount: 1 }),
        &[(&ssk, &spk)],
    );
    assert_eq!(r.apply(&late, 101, 0), Err(AccountError::OutOfScope));
}

#[test]
fn vault_transfers_wait_and_guardians_can_cancel_them() {
    let (mut r, a, (sk, pk), b) = setup();
    let (_, gpk) = key(6);
    let g = r.create(SuiteId::MlDsa65, &gpk, 0, 0);
    r.apply(
        &signed(
            op(
                a,
                0,
                Action::SetGuardians(GuardianSet {
                    guardians: vec![g],
                    threshold: 1,
                    delay_blocks: 50,
                }),
            ),
            &[(&sk, &pk)],
        ),
        1,
        0,
    )
    .unwrap();
    r.apply(
        &signed(
            op(
                a,
                1,
                Action::SetPolicy(Policy {
                    delay_above: 1_000,
                    delay_blocks: 100,
                    ..Policy::default()
                }),
            ),
            &[(&sk, &pk)],
        ),
        1,
        0,
    )
    .unwrap();
    r.apply(
        &signed(
            op(
                a,
                2,
                Action::Transfer {
                    to: b,
                    amount: 5_000,
                },
            ),
            &[(&sk, &pk)],
        ),
        10,
        0,
    )
    .unwrap();
    assert_eq!(r.get(&b).unwrap().balance, 0, "held, not sent");
    let early = signed(op(a, 3, Action::ReleasePending { id: 0 }), &[(&sk, &pk)]);
    assert_eq!(
        r.apply(&early, 50, 0),
        Err(AccountError::Pending("delay not over"))
    );
    r.guardian_cancel(&a, 0, &[g]).unwrap();
    assert_eq!(
        r.get(&a).unwrap().balance,
        1_000_000,
        "cancelled funds return"
    );
    // A second one is released after the delay.
    r.apply(
        &signed(
            op(
                a,
                3,
                Action::Transfer {
                    to: b,
                    amount: 2_000,
                },
            ),
            &[(&sk, &pk)],
        ),
        200,
        0,
    )
    .unwrap();
    r.apply(
        &signed(op(a, 4, Action::ReleasePending { id: 1 }), &[(&sk, &pk)]),
        300,
        0,
    )
    .unwrap();
    assert_eq!(r.get(&b).unwrap().balance, 2_000);
}

#[test]
fn guardians_recover_an_account_after_the_delay_and_the_owner_can_cancel() {
    let (mut r, a, (sk, pk), b) = setup();
    let (g1sk, g1pk) = key(7);
    let (g2sk, g2pk) = key(8);
    let g1 = r.create(SuiteId::MlDsa65, &g1pk, 0, 0);
    let g2 = r.create(SuiteId::MlDsa65, &g2pk, 0, 0);
    r.apply(
        &signed(
            op(
                a,
                0,
                Action::SetGuardians(GuardianSet {
                    guardians: vec![g1, g2],
                    threshold: 2,
                    delay_blocks: 100,
                }),
            ),
            &[(&sk, &pk)],
        ),
        1,
        0,
    )
    .unwrap();
    let (nsk, npk) = key(10);
    let approve = || Action::ApproveRecovery {
        target: a,
        suite: SuiteId::MlDsa65,
        public_key: npk.clone(),
    };
    r.apply(&signed(op(g1, 0, approve()), &[(&g1sk, &g1pk)]), 10, 0)
        .unwrap();
    r.apply(&signed(op(g2, 0, approve()), &[(&g2sk, &g2pk)]), 11, 0)
        .unwrap();
    let too_soon = signed(
        op(g1, 1, Action::FinalizeRecovery { target: a }),
        &[(&g1sk, &g1pk)],
    );
    assert_eq!(
        r.apply(&too_soon, 50, 0),
        Err(AccountError::Recovery("cancel window still open"))
    );
    // The owner is still around and cancels.
    r.apply(
        &signed(op(a, 1, Action::CancelRecovery), &[(&sk, &pk)]),
        60,
        0,
    )
    .unwrap();
    let nothing = signed(
        op(g1, 1, Action::FinalizeRecovery { target: a }),
        &[(&g1sk, &g1pk)],
    );
    assert_eq!(
        r.apply(&nothing, 500, 0),
        Err(AccountError::Recovery("no recovery in progress"))
    );
    // The owner really loses the key: guardians approve again and it completes.
    r.apply(&signed(op(g1, 1, approve()), &[(&g1sk, &g1pk)]), 600, 0)
        .unwrap();
    r.apply(&signed(op(g2, 1, approve()), &[(&g2sk, &g2pk)]), 601, 0)
        .unwrap();
    r.apply(
        &signed(
            op(g1, 2, Action::FinalizeRecovery { target: a }),
            &[(&g1sk, &g1pk)],
        ),
        701,
        0,
    )
    .unwrap();
    let old = signed(
        op(a, 2, Action::Transfer { to: b, amount: 1 }),
        &[(&sk, &pk)],
    );
    assert_eq!(r.apply(&old, 702, 0), Err(AccountError::UnknownKey));
    r.apply(
        &signed(
            op(a, 2, Action::Transfer { to: b, amount: 1 }),
            &[(&nsk, &npk)],
        ),
        702,
        0,
    )
    .unwrap();
}

#[test]
fn fees_never_exceed_the_quote_and_sponsors_enforce_their_caps() {
    let (mut r, a, (sk, pk), b) = setup();
    let over = signed(
        op(a, 0, Action::Transfer { to: b, amount: 1 }),
        &[(&sk, &pk)],
    );
    let before = r.get(&a).unwrap().clone();
    assert_eq!(r.apply(&over, 1, 1_001), Err(AccountError::FeeAboveMaximum));
    assert_eq!(r.get(&a).unwrap(), &before, "a refused op changes nothing");
    // Token fee: price 1 native = 2 token units, 5% margin.
    r.prices.insert(
        7,
        TokenPrice {
            native: 1,
            token: 2,
        },
    );
    r.token_balances.insert((a, 7), 1_000);
    let mut t = op(a, 0, Action::Transfer { to: b, amount: 1 });
    t.fee = FeeSpec::Token {
        token: 7,
        max_amount: 300,
    };
    r.apply(&signed(t, &[(&sk, &pk)]), 1, 100).unwrap();
    assert_eq!(
        r.token_balances[&(a, 7)],
        1_000 - 210,
        "100 native = 200 tokens + 5%"
    );
    // Sponsored: 150 per account per day.
    let (_, ppk) = key(11);
    let pmid = r.create(SuiteId::MlDsa65, &ppk, 0, 0);
    r.paymasters.insert(
        pmid,
        Paymaster {
            sponsored: vec![a],
            per_account_daily: 150,
            budget: 10_000,
            ..Paymaster::default()
        },
    );
    let sponsored = |n| {
        let mut s = op(a, n, Action::Transfer { to: b, amount: 1 });
        s.fee = FeeSpec::Sponsored { paymaster: pmid };
        signed(s, &[(&sk, &pk)])
    };
    r.apply(&sponsored(1), 2, 100).unwrap();
    assert_eq!(
        r.apply(&sponsored(2), 3, 100),
        Err(AccountError::SponsorRefused),
        "daily cap"
    );
    assert_eq!(r.paymasters[&pmid].budget, 9_900);
}

#[test]
fn a_replayed_op_is_refused() {
    let (mut r, a, (sk, pk), b) = setup();
    let o = signed(
        op(a, 0, Action::Transfer { to: b, amount: 1 }),
        &[(&sk, &pk)],
    );
    r.apply(&o, 1, 0).unwrap();
    assert_eq!(r.apply(&o, 2, 0), Err(AccountError::BadNonce));
}
