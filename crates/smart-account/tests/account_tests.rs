//! Smart accounts with real ML-DSA-65 keys (Master Prompts 13 §2, 22).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::many_single_char_names,
    clippy::cast_possible_truncation
)]

use maya_amount::Amount;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SlhDsaShake256f, SuiteId};
use maya_smart_account::account::{AccountError, MAX_SIGNATURES};
use maya_smart_account::fees::{FeeSpec, Paymaster, TokenPrice};
use maya_smart_account::op::{Action, Op};
use maya_smart_account::policy::{BLOCKS_PER_DAY, Policy, SessionScope};
use maya_smart_account::recovery::{
    GuardianSet, MAX_DELAY_BLOCKS, MAX_GUARDIANS, MIN_DELAY_BLOCKS,
};
use maya_smart_account::{AccountId, Registry, key_hash};
use proptest::prelude::*;

type Sk = <MlDsa65 as SignatureSuite>::SigningKey;

/// A legal recovery delay: the shortest, one epoch.
const DELAY: u64 = MIN_DELAY_BLOCKS;

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
        fee: FeeSpec::Native {
            max_fee: Amount::from_u64(1_000),
        },
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
    let a = r.create(SuiteId::MlDsa65, &alice.1, 0, Amount::from_u64(1_000_000));
    let bob = key(2);
    let b = r.create(SuiteId::MlDsa65, &bob.1, 0, Amount::from_u64(0));
    (r, a, alice, b)
}

#[test]
fn a_transfer_carries_a_key_hash_not_a_public_key() {
    let (mut r, a, (sk, pk), b) = setup();
    let o = signed(
        op(
            a,
            0,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(10),
            },
        ),
        &[(&sk, &pk)],
    );
    let with_key = o.encoded_len() + pk.len();
    println!(
        "ML-DSA-65 transfer: {} bytes on the wire; {} if it carried the public key",
        o.encoded_len(),
        with_key
    );
    // 200 bytes of overhead, plus 48 because the amount and the fee ceiling are
    // 32 bytes each (256-bit, ADR-046) instead of 8.
    assert!(o.encoded_len() < 3_309 + 200 + 48);
    assert_eq!(with_key - o.encoded_len(), 1_952);
    r.apply(&o, 1, Amount::from_u64(100)).unwrap();
    assert_eq!(r.get(&b).unwrap().balance, Amount::from_u64(10));
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
    r.apply(
        &signed(op(a, 0, rotate), &[(&old_sk, &old_pk)]),
        1,
        Amount::from_u64(10),
    )
    .unwrap();
    let stale = signed(
        op(
            a,
            1,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(5),
            },
        ),
        &[(&old_sk, &old_pk)],
    );
    assert_eq!(
        r.apply(&stale, 2, Amount::from_u64(10)),
        Err(AccountError::UnknownKey)
    );
    let fresh = signed(
        op(
            a,
            1,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(5),
            },
        ),
        &[(&new_sk, &new_pk)],
    );
    r.apply(&fresh, 2, Amount::from_u64(10)).unwrap();
    assert_eq!(r.get(&a).unwrap().id, a, "the address did not change");
    assert_eq!(r.get(&b).unwrap().balance, Amount::from_u64(5));
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
        Amount::from_u64(10),
    )
    .unwrap();
    r.set_threshold(&a, 2);
    let one = signed(
        op(
            a,
            1,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk1, &pk1)],
    );
    assert_eq!(
        r.apply(&one, 2, Amount::from_u64(10)),
        Err(AccountError::BelowThreshold)
    );
    let two = signed(
        op(
            a,
            1,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk1, &pk1), (&sk2, &pk2)],
    );
    r.apply(&two, 2, Amount::from_u64(10)).unwrap();
}

#[test]
fn validation_cost_is_bounded_before_any_signature_is_verified() {
    let (mut r, a, _, b) = setup();
    let mut o = op(
        a,
        0,
        Action::Transfer {
            to: b,
            amount: Amount::from_u64(1),
        },
    );
    o.signatures = (0..=MAX_SIGNATURES as u8)
        .map(|i| ([i; 32], vec![0u8; 3_309]))
        .collect();
    assert_eq!(
        r.apply(&o, 1, Amount::from_u64(10)),
        Err(AccountError::ValidationBudget)
    );
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
            Amount::from_u64(0),
        )
        .unwrap();
        nonce += 1;
    }
    let mut heavy = op(
        a,
        nonce,
        Action::Transfer {
            to: b,
            amount: Amount::from_u64(1),
        },
    );
    heavy.signatures = hashes.into_iter().map(|h| (h, vec![0u8; 49_856])).collect();
    assert_eq!(
        r.apply(&heavy, 1, Amount::from_u64(0)),
        Err(AccountError::ValidationBudget)
    );
}

#[test]
fn policies_limit_spending_and_recipients() {
    let (mut r, a, (sk, pk), b) = setup();
    let (_, cpk) = key(4);
    let c = r.create(SuiteId::MlDsa65, &cpk, 0, Amount::from_u64(0));
    let policy = Policy {
        daily_limit: Amount::from_u64(1_000),
        per_recipient_daily: Amount::from_u64(600),
        allow_list: vec![b],
        ..Policy::default()
    };
    r.apply(
        &signed(op(a, 0, Action::SetPolicy(policy)), &[(&sk, &pk)]),
        1,
        Amount::from_u64(0),
    )
    .unwrap();
    let to_c = signed(
        op(
            a,
            1,
            Action::Transfer {
                to: c,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk, &pk)],
    );
    assert!(matches!(
        r.apply(&to_c, 2, Amount::from_u64(0)),
        Err(AccountError::Policy(_))
    ));
    r.apply(
        &signed(
            op(
                a,
                1,
                Action::Transfer {
                    to: b,
                    amount: Amount::from_u64(600),
                },
            ),
            &[(&sk, &pk)],
        ),
        2,
        Amount::from_u64(0),
    )
    .unwrap();
    let over = signed(
        op(
            a,
            2,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk, &pk)],
    );
    assert_eq!(
        r.apply(&over, 3, Amount::from_u64(0)),
        Err(AccountError::Policy("per-recipient daily limit"))
    );
    // Next day the limits reset.
    r.apply(
        &signed(
            op(
                a,
                2,
                Action::Transfer {
                    to: b,
                    amount: Amount::from_u64(600),
                },
            ),
            &[(&sk, &pk)],
        ),
        43_200 + 1,
        Amount::from_u64(0),
    )
    .unwrap();
}

#[test]
fn session_keys_are_scoped_and_expire() {
    let (mut r, a, (sk, pk), b) = setup();
    let (ssk, spk) = key(5);
    let scope = SessionScope {
        recipients: vec![b],
        max_amount: Amount::from_u64(50),
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
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(
            op(
                a,
                1,
                Action::Transfer {
                    to: b,
                    amount: Amount::from_u64(50),
                },
            ),
            &[(&ssk, &spk)],
        ),
        10,
        Amount::from_u64(0),
    )
    .unwrap();
    let too_much = signed(
        op(
            a,
            2,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(51),
            },
        ),
        &[(&ssk, &spk)],
    );
    assert_eq!(
        r.apply(&too_much, 10, Amount::from_u64(0)),
        Err(AccountError::OutOfScope)
    );
    let admin = signed(
        op(a, 2, Action::SetPolicy(Policy::default())),
        &[(&ssk, &spk)],
    );
    assert_eq!(
        r.apply(&admin, 10, Amount::from_u64(0)),
        Err(AccountError::OutOfScope)
    );
    let late = signed(
        op(
            a,
            2,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&ssk, &spk)],
    );
    assert_eq!(
        r.apply(&late, 101, Amount::from_u64(0)),
        Err(AccountError::OutOfScope)
    );
}

#[test]
fn vault_transfers_wait_and_guardians_can_cancel_them() {
    let (mut r, a, (sk, pk), b) = setup();
    let (_, gpk) = key(6);
    let g = r.create(SuiteId::MlDsa65, &gpk, 0, Amount::from_u64(0));
    r.apply(
        &signed(
            op(
                a,
                0,
                Action::SetGuardians(GuardianSet {
                    guardians: vec![g],
                    threshold: 1,
                    delay_blocks: DELAY,
                }),
            ),
            &[(&sk, &pk)],
        ),
        1,
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(
            op(
                a,
                1,
                Action::SetPolicy(Policy {
                    delay_above: Amount::from_u64(1_000),
                    delay_blocks: 100,
                    ..Policy::default()
                }),
            ),
            &[(&sk, &pk)],
        ),
        1,
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(
            op(
                a,
                2,
                Action::Transfer {
                    to: b,
                    amount: Amount::from_u64(5_000),
                },
            ),
            &[(&sk, &pk)],
        ),
        10,
        Amount::from_u64(0),
    )
    .unwrap();
    assert_eq!(
        r.get(&b).unwrap().balance,
        Amount::from_u64(0),
        "held, not sent"
    );
    let early = signed(op(a, 3, Action::ReleasePending { id: 0 }), &[(&sk, &pk)]);
    assert_eq!(
        r.apply(&early, 50, Amount::from_u64(0)),
        Err(AccountError::Pending("delay not over"))
    );
    r.guardian_cancel(&a, 0, &[g]).unwrap();
    assert_eq!(
        r.get(&a).unwrap().balance,
        Amount::from_u64(1_000_000),
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
                    amount: Amount::from_u64(2_000),
                },
            ),
            &[(&sk, &pk)],
        ),
        200,
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(op(a, 4, Action::ReleasePending { id: 1 }), &[(&sk, &pk)]),
        300,
        Amount::from_u64(0),
    )
    .unwrap();
    assert_eq!(r.get(&b).unwrap().balance, Amount::from_u64(2_000));
}

#[test]
fn guardians_recover_an_account_after_the_delay_and_the_owner_can_cancel() {
    let (mut r, a, (sk, pk), b) = setup();
    let (g1sk, g1pk) = key(7);
    let (g2sk, g2pk) = key(8);
    let g1 = r.create(SuiteId::MlDsa65, &g1pk, 0, Amount::from_u64(0));
    let g2 = r.create(SuiteId::MlDsa65, &g2pk, 0, Amount::from_u64(0));
    r.apply(
        &signed(
            op(
                a,
                0,
                Action::SetGuardians(GuardianSet {
                    guardians: vec![g1, g2],
                    threshold: 2,
                    delay_blocks: DELAY,
                }),
            ),
            &[(&sk, &pk)],
        ),
        1,
        Amount::from_u64(0),
    )
    .unwrap();
    let (nsk, npk) = key(10);
    let approve = || Action::ApproveRecovery {
        target: a,
        suite: SuiteId::MlDsa65,
        public_key: npk.clone(),
    };
    r.apply(
        &signed(op(g1, 0, approve()), &[(&g1sk, &g1pk)]),
        10,
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(op(g2, 0, approve()), &[(&g2sk, &g2pk)]),
        11,
        Amount::from_u64(0),
    )
    .unwrap();
    let too_soon = signed(
        op(g1, 1, Action::FinalizeRecovery { target: a }),
        &[(&g1sk, &g1pk)],
    );
    assert_eq!(
        r.apply(&too_soon, 50, Amount::from_u64(0)),
        Err(AccountError::Recovery("cancel window still open"))
    );
    // The owner is still around and cancels.
    r.apply(
        &signed(op(a, 1, Action::CancelRecovery), &[(&sk, &pk)]),
        60,
        Amount::from_u64(0),
    )
    .unwrap();
    let nothing = signed(
        op(g1, 1, Action::FinalizeRecovery { target: a }),
        &[(&g1sk, &g1pk)],
    );
    assert_eq!(
        r.apply(&nothing, 500, Amount::from_u64(0)),
        Err(AccountError::Recovery("no recovery in progress"))
    );
    // The owner really loses the key: guardians approve again and it completes.
    r.apply(
        &signed(op(g1, 1, approve()), &[(&g1sk, &g1pk)]),
        600,
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(op(g2, 1, approve()), &[(&g2sk, &g2pk)]),
        601,
        Amount::from_u64(0),
    )
    .unwrap();
    r.apply(
        &signed(
            op(g1, 2, Action::FinalizeRecovery { target: a }),
            &[(&g1sk, &g1pk)],
        ),
        601 + DELAY,
        Amount::from_u64(0),
    )
    .unwrap();
    let old = signed(
        op(
            a,
            2,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk, &pk)],
    );
    assert_eq!(
        r.apply(&old, 602 + DELAY, Amount::from_u64(0)),
        Err(AccountError::UnknownKey)
    );
    r.apply(
        &signed(
            op(
                a,
                2,
                Action::Transfer {
                    to: b,
                    amount: Amount::from_u64(1),
                },
            ),
            &[(&nsk, &npk)],
        ),
        602 + DELAY,
        Amount::from_u64(0),
    )
    .unwrap();
}

#[test]
fn fees_never_exceed_the_quote_and_sponsors_enforce_their_caps() {
    let (mut r, a, (sk, pk), b) = setup();
    let over = signed(
        op(
            a,
            0,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk, &pk)],
    );
    let before = r.get(&a).unwrap().clone();
    assert_eq!(
        r.apply(&over, 1, Amount::from_u64(1_001)),
        Err(AccountError::FeeAboveMaximum)
    );
    assert_eq!(r.get(&a).unwrap(), &before, "a refused op changes nothing");
    // Token fee: price 1 native = 2 token units, 5% margin.
    r.prices.insert(
        7,
        TokenPrice {
            native: Amount::from_u64(1),
            token: Amount::from_u64(2),
        },
    );
    r.token_balances.insert((a, 7), Amount::from_u64(1_000));
    let mut t = op(
        a,
        0,
        Action::Transfer {
            to: b,
            amount: Amount::from_u64(1),
        },
    );
    t.fee = FeeSpec::Token {
        token: 7,
        max_amount: Amount::from_u64(300),
    };
    r.apply(&signed(t, &[(&sk, &pk)]), 1, Amount::from_u64(100))
        .unwrap();
    assert_eq!(
        r.token_balances[&(a, 7)],
        Amount::from_u64(1_000 - 210),
        "100 native = 200 tokens + 5%"
    );
    // Sponsored: 150 per account per day.
    let (_, ppk) = key(11);
    let pmid = r.create(SuiteId::MlDsa65, &ppk, 0, Amount::from_u64(0));
    r.paymasters.insert(
        pmid,
        Paymaster {
            sponsored: vec![a],
            per_account_daily: Amount::from_u64(150),
            budget: Amount::from_u64(10_000),
            ..Paymaster::default()
        },
    );
    let sponsored = |n| {
        let mut s = op(
            a,
            n,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        );
        s.fee = FeeSpec::Sponsored { paymaster: pmid };
        signed(s, &[(&sk, &pk)])
    };
    r.apply(&sponsored(1), 2, Amount::from_u64(100)).unwrap();
    assert_eq!(
        r.apply(&sponsored(2), 3, Amount::from_u64(100)),
        Err(AccountError::SponsorRefused),
        "daily cap"
    );
    assert_eq!(r.paymasters[&pmid].budget, Amount::from_u64(9_900));
}

#[test]
fn a_replayed_op_is_refused() {
    let (mut r, a, (sk, pk), b) = setup();
    let o = signed(
        op(
            a,
            0,
            Action::Transfer {
                to: b,
                amount: Amount::from_u64(1),
            },
        ),
        &[(&sk, &pk)],
    );
    r.apply(&o, 1, Amount::from_u64(0)).unwrap();
    assert_eq!(
        r.apply(&o, 2, Amount::from_u64(0)),
        Err(AccountError::BadNonce)
    );
}

// ---- Recovery corrections (ADR-086 §4, F1–F6) ----

/// An account and the key that signs for it.
struct Party {
    id: AccountId,
    sk: Sk,
    pk: Vec<u8>,
}

fn party(r: &mut Registry, seed: u8, balance: u64) -> Party {
    let (sk, pk) = key(seed);
    let id = r.create(SuiteId::MlDsa65, &pk, 0, Amount::from_u64(balance));
    Party { id, sk, pk }
}

/// Signs `action` as `p` at its next nonce and applies it, fee zero.
fn act(r: &mut Registry, p: &Party, action: Action, height: u64) -> Result<(), AccountError> {
    let nonce = r.get(&p.id).unwrap().nonce;
    let o = signed(op(p.id, nonce, action), &[(&p.sk, &p.pk)]);
    r.apply(&o, height, Amount::from_u64(0))
}

/// Owner (1,000,000), payee (0) and `n` guardians, the set installed at
/// height 1 with `threshold` and [`DELAY`].
fn guarded(n: u8, threshold: usize) -> (Registry, Party, Party, Vec<Party>) {
    let mut r = Registry::new();
    let owner = party(&mut r, 1, 1_000_000);
    let payee = party(&mut r, 2, 0);
    let gs: Vec<Party> = (0..n).map(|i| party(&mut r, 20 + i, 0)).collect();
    let set = GuardianSet {
        guardians: gs.iter().map(|g| g.id).collect(),
        threshold,
        delay_blocks: DELAY,
    };
    act(&mut r, &owner, Action::SetGuardians(set), 1).unwrap();
    (r, owner, payee, gs)
}

fn approve(target: &Party, new: &(Sk, Vec<u8>)) -> Action {
    Action::ApproveRecovery {
        target: target.id,
        suite: SuiteId::MlDsa65,
        public_key: new.1.clone(),
    }
}

fn finalize(target: &Party) -> Action {
    Action::FinalizeRecovery { target: target.id }
}

fn owner_hashes(r: &Registry, id: &AccountId) -> Vec<[u8; 32]> {
    r.get(id).unwrap().keys.iter().map(|k| k.hash).collect()
}

/// F1: whoever holds the key cannot swap the guardians out from under a
/// recovery; a change after the first waits the current delay.
#[test]
fn a_later_guardian_change_waits_out_the_current_delay() {
    let (mut r, owner, _, gs) = guarded(3, 2);
    let newcomer = party(&mut r, 40, 0);
    let set = GuardianSet {
        guardians: vec![newcomer.id],
        threshold: 1,
        delay_blocks: DELAY,
    };
    act(&mut r, &owner, Action::SetGuardians(set.clone()), 10).unwrap();
    let k = key(50);
    assert_eq!(
        act(&mut r, &newcomer, approve(&owner, &k), 11),
        Err(AccountError::Recovery("not a guardian of this account")),
        "the new set is queued, not live"
    );
    act(&mut r, &gs[0], approve(&owner, &k), 12).unwrap();
    act(&mut r, &newcomer, approve(&owner, &k), 10 + DELAY).unwrap();
    assert_eq!(
        r.get(&owner.id).unwrap().guardians,
        set,
        "live after the delay"
    );
}

/// F2: recovery removes every key the thief could hold, session keys too.
#[test]
fn recovery_drops_every_key_including_session_keys() {
    let (mut r, owner, payee, gs) = guarded(1, 1);
    let (ssk, spk) = key(41);
    let scope = SessionScope {
        recipients: vec![payee.id],
        max_amount: Amount::from_u64(1_000),
        expires_at: 1_000_000,
    };
    let add = Action::AddSessionKey {
        suite: SuiteId::MlDsa65,
        public_key: spk.clone(),
        scope,
    };
    act(&mut r, &owner, add, 2).unwrap();
    let k = key(51);
    act(&mut r, &gs[0], approve(&owner, &k), 3).unwrap();
    act(&mut r, &gs[0], finalize(&owner), 3 + DELAY).unwrap();
    let pay = Action::Transfer {
        to: payee.id,
        amount: Amount::from_u64(1),
    };
    let nonce = r.get(&owner.id).unwrap().nonce;
    let by_session = signed(op(owner.id, nonce, pay), &[(&ssk, &spk)]);
    assert_eq!(
        r.apply(&by_session, 4 + DELAY, Amount::from_u64(0)),
        Err(AccountError::UnknownKey)
    );
    assert_eq!(
        owner_hashes(&r, &owner.id),
        vec![key_hash(SuiteId::MlDsa65, &k.1)]
    );
}

/// F4: approvals from guardians no longer in the set do not count.
#[test]
fn removed_guardians_approvals_do_not_count() {
    let (mut r, owner, _, gs) = guarded(3, 2);
    let k = key(52);
    act(&mut r, &gs[0], approve(&owner, &k), 5).unwrap();
    let g4 = party(&mut r, 42, 0);
    let set = GuardianSet {
        guardians: vec![gs[1].id, gs[2].id, g4.id],
        threshold: 2,
        delay_blocks: DELAY,
    };
    act(&mut r, &owner, Action::SetGuardians(set), 10).unwrap();
    act(&mut r, &gs[1], approve(&owner, &k), 20).unwrap();
    let before = owner_hashes(&r, &owner.id);
    assert_eq!(
        act(&mut r, &gs[1], finalize(&owner), 20 + DELAY),
        Err(AccountError::Recovery("no guardian quorum yet")),
        "gs[0] left the set; one current approval is below threshold"
    );
    assert_eq!(owner_hashes(&r, &owner.id), before);
}

/// F5: transfers the thief queued while the owner was locked out return to
/// the balance when recovery completes.
#[test]
fn recovery_cancels_pending_transfers_a_thief_queued() {
    let (mut r, owner, payee, gs) = guarded(1, 1);
    let policy = Policy {
        delay_above: Amount::from_u64(1_000),
        delay_blocks: 10 * DELAY,
        ..Policy::default()
    };
    act(&mut r, &owner, Action::SetPolicy(policy), 2).unwrap();
    let theft = Action::Transfer {
        to: payee.id,
        amount: Amount::from_u64(900_000),
    };
    act(&mut r, &owner, theft, 3).unwrap();
    let k = key(53);
    act(&mut r, &gs[0], approve(&owner, &k), 4).unwrap();
    act(&mut r, &gs[0], finalize(&owner), 4 + DELAY).unwrap();
    let a = r.get(&owner.id).unwrap();
    assert!(a.pending.is_empty(), "nothing left to release");
    assert_eq!(a.balance, Amount::from_u64(1_000_000));
    assert_eq!(r.get(&payee.id).unwrap().balance, Amount::from_u64(0));
}

/// F3: guardian sets are bounded; each malformed set is refused.
#[test]
fn guardian_sets_are_validated() {
    let mut r = Registry::new();
    let owner = party(&mut r, 1, 0);
    let gs: Vec<AccountId> = (0..8).map(|i| party(&mut r, 20 + i, 0).id).collect();
    let set = |guardians: Vec<AccountId>, threshold, delay_blocks| GuardianSet {
        guardians,
        threshold,
        delay_blocks,
    };
    let bad = [
        (set(vec![], 0, DELAY), "guardian count must be 1 to 7"),
        (
            set(gs[..=MAX_GUARDIANS].to_vec(), 5, DELAY),
            "guardian count must be 1 to 7",
        ),
        (
            set(vec![gs[0], gs[0]], 2, DELAY),
            "guardians must be distinct",
        ),
        (
            set(vec![owner.id], 1, DELAY),
            "an account cannot guard itself",
        ),
        (
            set(vec![gs[0]], 0, DELAY),
            "threshold must be a strict majority",
        ),
        (
            set(gs[..4].to_vec(), 2, DELAY),
            "threshold must be a strict majority",
        ),
        (
            set(gs[..2].to_vec(), 3, DELAY),
            "threshold must be a strict majority",
        ),
        (
            set(vec![gs[0]], 1, MIN_DELAY_BLOCKS - 1),
            "delay out of bounds",
        ),
        (
            set(vec![gs[0]], 1, MAX_DELAY_BLOCKS + 1),
            "delay out of bounds",
        ),
    ];
    for (s, why) in bad {
        assert_eq!(
            act(&mut r, &owner, Action::SetGuardians(s.clone()), 1),
            Err(AccountError::Recovery(why)),
            "{s:?}"
        );
    }
    assert!(r.get(&owner.id).unwrap().guardians.guardians.is_empty());
    let edge = set(gs[..MAX_GUARDIANS].to_vec(), 4, MAX_DELAY_BLOCKS);
    act(&mut r, &owner, Action::SetGuardians(edge), 1).unwrap();
    let low = set(vec![gs[0]], 1, MIN_DELAY_BLOCKS);
    act(&mut r, &owner, Action::SetGuardians(low), 2).unwrap();
}

/// The bounds mirror the node's; a change there must be made here too.
#[test]
fn delay_bounds_match_the_node_constants() {
    let vault = include_str!("../../node/src/state/vault.rs");
    assert!(vault.contains(&format!(
        "pub const MAX_DELAY_BLOCKS: u64 = {};",
        group(MAX_DELAY_BLOCKS)
    )));
    let committee = include_str!("../../../bins/genesis-ceremony/src/committee.rs");
    assert!(committee.contains(&format!(
        "pub const DEFAULT_EPOCH_BLOCKS: u64 = {};",
        group(MIN_DELAY_BLOCKS)
    )));
}

/// `1000000` as `1_000_000`, the way the source writes it.
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(c);
    }
    out
}

/// F7, documented not decided: the policy day assumes 2 s blocks, while the
/// node's only block-time constant is `TARGET_BLOCK_TIME`, 15 s. Changing either
/// is a question for the owner (ADR-086 §4); this pins today's values.
#[test]
fn policy_day_length_is_pinned_pending_a_decision() {
    assert_eq!(BLOCKS_PER_DAY, 43_200, "86_400 s / 2 s");
    let difficulty = include_str!("../../node/src/consensus/difficulty.rs");
    assert!(difficulty.contains("pub const TARGET_BLOCK_TIME: u64 = 15;"));
}

/// F6: a hostile guardian proposing its own key first does not stop the
/// others recovering to theirs.
#[test]
fn one_hostile_guardian_cannot_block_an_honest_majority() {
    let (mut r, owner, _, gs) = guarded(3, 2);
    let evil = key(54);
    let good = key(55);
    act(&mut r, &gs[0], approve(&owner, &evil), 10).unwrap();
    act(&mut r, &gs[1], approve(&owner, &good), 11).unwrap();
    act(&mut r, &gs[2], approve(&owner, &good), 12).unwrap();
    assert_eq!(
        act(&mut r, &gs[0], finalize(&owner), 11 + DELAY),
        Err(AccountError::Recovery("cancel window still open"))
    );
    act(&mut r, &gs[0], finalize(&owner), 12 + DELAY).unwrap();
    assert_eq!(
        owner_hashes(&r, &owner.id),
        vec![key_hash(SuiteId::MlDsa65, &good.1)]
    );
}

/// F6: an approval follows the guardian's latest proposal, and can be
/// withdrawn; either drops a request below quorum and restarts its clock.
#[test]
fn approvals_follow_the_latest_proposal_and_can_be_withdrawn() {
    let (mut r, owner, _, gs) = guarded(3, 2);
    let k1 = key(56);
    let k2 = key(57);
    act(&mut r, &gs[0], approve(&owner, &k1), 10).unwrap();
    act(&mut r, &gs[1], approve(&owner, &k1), 10).unwrap();
    act(&mut r, &gs[1], approve(&owner, &k2), 11).unwrap();
    assert_eq!(
        act(&mut r, &gs[2], finalize(&owner), 10 + DELAY),
        Err(AccountError::Recovery("no guardian quorum yet")),
        "gs[1] moved to k2"
    );
    act(&mut r, &gs[1], approve(&owner, &k1), 20).unwrap();
    let withdraw = Action::WithdrawApproval { target: owner.id };
    act(&mut r, &gs[1], withdraw.clone(), 21).unwrap();
    assert_eq!(
        act(&mut r, &gs[1], withdraw, 22),
        Err(AccountError::Recovery("no approval to withdraw"))
    );
    assert_eq!(
        act(&mut r, &gs[2], finalize(&owner), 20 + DELAY),
        Err(AccountError::Recovery("no guardian quorum yet"))
    );
    act(&mut r, &gs[2], approve(&owner, &k1), 30).unwrap();
    assert_eq!(
        act(&mut r, &gs[2], finalize(&owner), 29 + DELAY),
        Err(AccountError::Recovery("cancel window still open")),
        "the clock restarted at the new quorum"
    );
    act(&mut r, &gs[2], finalize(&owner), 30 + DELAY).unwrap();
    assert_eq!(
        owner_hashes(&r, &owner.id),
        vec![key_hash(SuiteId::MlDsa65, &k1.1)]
    );
}

/// One step of the random walk below.
#[derive(Clone, Debug)]
enum Step {
    Pay(u64),
    Release(u64),
    CancelPending(u64),
    CancelRecovery,
    Approve(usize, usize),
    Withdraw(usize),
    Finalize,
    Wait(u64),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (1..400_000u64).prop_map(Step::Pay),
        (0..4u64).prop_map(Step::Release),
        (0..4u64).prop_map(Step::CancelPending),
        Just(Step::CancelRecovery),
        (0..7usize, 0..3usize).prop_map(|(g, k)| Step::Approve(g, k)),
        (0..7usize).prop_map(Step::Withdraw),
        Just(Step::Finalize),
        (0..2 * DELAY).prop_map(Step::Wait),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Fewer than `threshold` guardians acting, whatever they and the owner
    /// do and however long they wait, never change the owner's keys; and
    /// value is only ever moved, never made or lost.
    #[test]
    fn below_threshold_guardians_never_change_keys_and_value_is_conserved(
        n in 1u8..=7,
        steps in proptest::collection::vec(step(), 1..30),
    ) {
        let threshold = usize::from(n) / 2 + 1;
        let (mut r, owner, payee, gs) = guarded(n, threshold);
        let policy = Policy {
            delay_above: Amount::from_u64(100_000),
            delay_blocks: DELAY,
            ..Policy::default()
        };
        act(&mut r, &owner, Action::SetPolicy(policy), 1).unwrap();
        let acting = &gs[..threshold - 1];
        let keys = [key(60), key(61), key(62)];
        let before = owner_hashes(&r, &owner.id);
        let total = |r: &Registry| {
            let a = r.get(&owner.id).unwrap();
            let held = a
                .pending
                .values()
                .fold(Amount::from_u64(0), |s, p| s.saturating_add(p.amount));
            a.balance
                .saturating_add(r.get(&payee.id).unwrap().balance)
                .saturating_add(held)
        };
        let start = total(&r);
        let mut h = 2;
        for s in steps {
            let to = payee.id;
            let pick = |g: usize| &acting[g % acting.len()];
            // Refusals are expected; only the invariants matter.
            let _ = match s {
                Step::Pay(v) => {
                    let amount = Amount::from_u64(v);
                    act(&mut r, &owner, Action::Transfer { to, amount }, h)
                }
                Step::Release(id) => act(&mut r, &owner, Action::ReleasePending { id }, h),
                Step::CancelPending(id) => act(&mut r, &owner, Action::CancelPending { id }, h),
                Step::CancelRecovery => act(&mut r, &owner, Action::CancelRecovery, h),
                Step::Approve(g, k) if !acting.is_empty() => {
                    act(&mut r, pick(g), approve(&owner, &keys[k]), h)
                }
                Step::Withdraw(g) if !acting.is_empty() => {
                    let target = owner.id;
                    act(&mut r, pick(g), Action::WithdrawApproval { target }, h)
                }
                Step::Finalize => act(&mut r, &payee, finalize(&owner), h),
                Step::Wait(d) => {
                    h += d;
                    Ok(())
                }
                Step::Approve(..) | Step::Withdraw(_) => Ok(()),
            };
            prop_assert_eq!(owner_hashes(&r, &owner.id), before.clone());
            prop_assert_eq!(total(&r), start);
        }
    }
}
