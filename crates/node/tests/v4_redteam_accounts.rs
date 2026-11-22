//! Red team for smart-account policy (ADR-135 P-5, bit 45, V4-V1): the best
//! attacks found against session keys and the spending window, each pinned as
//! a test. Fixtures: `common/account_scene.rs`, shared with
//! `account_policy_tests.rs`, which holds the per-rule cases (cap, expiry,
//! recipients, revocation, budget, A->B->A for a key granted under A).
//!
//! Tried here:
//!
//! - **Value without outputs.** The window counts outputs (`vault_outflow`).
//!   A limited account that funds a channel or locks stake moves value the
//!   window never sees; with a channel to a colluding counterparty that is a
//!   withdrawal. On the full v4 fork the same shape is `CreateSchedule` with
//!   `first_due = height + 1` (#502), name fees (#493) and the asset
//!   registration fee (#487). Fixed: a limited account sends only plain
//!   transfers and account actions (AA-9), fail-closed for later kinds.
//! - **A session key signing any other kind** (S-1): refused for every kind
//!   but `Transfer`. The v4 forms are not on this base; the same `matches!`
//!   refuses them, and the integration branch must add each to the list.
//! - **Two limits in two blocks.** The window is aligned, not sliding (AA-9):
//!   the last block of one window and the first of the next together move
//!   twice the limit. Pinned as documented behaviour.
//! - **Replay** of a session frame: refused on its nonce.
//! - **A key granted under B revived by A->B->A**: void once A returns.
//! - **Passing the cap by repeating frames** (PR #548 review): `max_amount`
//!   is the key's total, and a frame the builder skips gives its charge back.
//! - **A guardian's vault cancel during a halt** (PR #548 review): the halt
//!   blocks lifting the guardian's limit, so the window must not block the
//!   never-halted cancel.
//!
//! Not testable on this base: a v13 sponsored frame (#501) wrapping a
//! session-signed v9 frame. #501's SP-1 refuses a v13 inner frame, not a v9
//! one, so the inner frame's session check is what must hold; that case
//! belongs to the postgen/v4 integration branch.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/account_scene.rs"]
mod account_scene;

use account_scene::*;
use custom_l1_node::core::{ChannelOpen, StakeLock};

fn open_channel(funding: u64) -> TxKind {
    TxKind::OpenChannel(ChannelOpen {
        counterparty: STRANGER,
        funding: U256::from(funding),
        dispute_window: 50,
    })
}

fn lock_stake(amount: u64) -> TxKind {
    TxKind::LockStake(StakeLock {
        amount: U256::from(amount),
        unlock_height: 10_000,
    })
}

#[test]
fn a_limited_account_cannot_move_value_without_outputs() {
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(CAP),
    })
    .unwrap();
    // Half the balance into a channel to a colluding counterparty, and half
    // into a stake lock: no outputs, so the window would charge nothing.
    refused(&mut s, "daily limit", |s| {
        let tx = s.as_owner(open_channel(FUNDS / 2), vec![]);
        s.chain.send(tx)
    });
    refused(&mut s, "daily limit", |s| {
        let tx = s.as_owner(lock_stake(FUNDS / 2), vec![]);
        s.chain.send(tx)
    });
    // The control: the same channel opens once the limit is lifted, so the
    // refusal above is the window's, not the channel's.
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::zero(),
    })
    .unwrap();
    let tx = s.as_owner(open_channel(FUNDS / 2), vec![]);
    s.chain.send(tx).expect("no limit, the channel opens");
    assert_eq!(s.chain.balance(&s.address), U256::from(FUNDS / 2));
}

#[test]
fn a_session_key_signs_no_kind_but_a_plain_transfer() {
    let mut s = Scene::opted_in();
    for kind in [open_channel(CAP), lock_stake(CAP)] {
        refused(&mut s, "only plain transfers", |s| {
            let tx = s.as_session(kind, vec![]);
            s.chain.send(tx)
        });
    }
}

#[test]
fn an_aligned_window_lets_two_limits_land_in_two_consecutive_blocks() {
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(CAP),
    })
    .unwrap();
    // Owner-signed: the session key's own expiry is long past by then.
    let owner_pay = |s: &mut Scene| {
        let tx = s.as_owner(TxKind::Transfer, pay_to(STRANGER, CAP));
        s.chain.send(tx)
    };
    s.chain.next_at(WINDOW_BLOCKS - 1);
    owner_pay(&mut s).expect("the last block of window 0");
    owner_pay(&mut s).expect("the first block of window 1: AA-9 is aligned, not sliding");
    assert_eq!(s.chain.balance(&STRANGER), U256::from(2 * CAP));
}

#[test]
fn a_replayed_session_frame_is_refused_on_its_nonce() {
    let mut s = Scene::opted_in();
    let tx = s.as_session(TxKind::Transfer, pay_to(PAYEE, 1));
    s.chain.send(tx.clone()).unwrap();
    refused(&mut s, "invalid nonce", |s| s.chain.send(tx));
    assert_eq!(s.chain.balance(&PAYEE), U256::one());
}

#[test]
fn a_key_granted_under_b_is_void_once_a_returns() {
    // A grants nothing here: B grants the session key, then the account goes
    // back to A. The key's `granted_by` is B, so it must not count under A.
    let mut s = Scene::opted_in();
    let b_key = device(0x0A);
    let b = session_policy(&b_key);
    s.owner_does(AccountAction::AdoptPolicy(b.clone()))
        .expect("A -> B");
    let as_b = |s: &Scene, kind: TxKind| {
        v9_frame(
            s.address,
            &b,
            kind,
            vec![],
            s.chain.nonce(&s.address),
            |m| {
                vec![Approval {
                    index: 0,
                    signature: MlDsa65::sign(&b_key, m).unwrap(),
                }]
            },
        )
    };
    let grant = AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(CAP, EXPIRES, vec![PAYEE]),
    };
    let tx = as_b(&s, account(grant));
    s.chain.send(tx).expect("B grants the session key");
    s.session_pays(PAYEE, 1).expect("live under B");
    let tx = as_b(
        &s,
        account(AccountAction::AdoptPolicy(s.owner_policy.clone())),
    );
    s.chain.send(tx).expect("B -> A");
    refused(&mut s, "authority moved", |s| s.session_pays(PAYEE, 1));
}

#[test]
fn a_session_key_cannot_pass_its_cap_by_repeating_frames() {
    // `max_amount` is the key's total, not each frame's: without a window a
    // leaked key would otherwise sign one capped frame per nonce until expiry.
    let mut s = Scene::opted_in();
    s.session_pays(PAYEE, CAP - 1).expect("under the cap");
    s.session_pays(PAYEE, 1)
        .expect("exactly the cap, over two frames");
    refused(&mut s, "max_amount", |s| s.session_pays(PAYEE, 1));
    assert_eq!(s.chain.balance(&PAYEE), U256::from(CAP));
    // Re-granting is the owner's explicit act and starts a fresh total.
    s.owner_does(AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(CAP, EXPIRES, vec![PAYEE]),
    })
    .expect("owner re-grants");
    s.session_pays(PAYEE, 1).expect("a fresh total");
}

#[test]
fn a_skipped_session_frame_leaves_no_cap_charge_in_the_builder() {
    // The session total is charged before the balance check, like the window.
    // A frame that then fails on balance must not use up the key's total.
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(2 * FUNDS, EXPIRES, vec![PAYEE]),
    })
    .unwrap();
    let broke = s.as_session(TxKind::Transfer, pay_to(PAYEE, 2 * FUNDS));
    let good = s.as_session(TxKind::Transfer, pay_to(PAYEE, FUNDS / 2));
    let kept = s
        .chain
        .db
        .select_applicable(
            vec![broke, good.clone()],
            s.chain.context(s.chain.height + 1),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(kept, vec![good], "the failed frame's charge was undone");
}

#[test]
fn a_guardian_with_a_daily_limit_cancels_a_vault_request_during_a_halt() {
    use custom_l1_node::state::invariant_guard::{BreakerRecord, Invariant, Module, guard_key};
    // The scene's account is the guardian G of another account's vault, and
    // has a daily limit. A thief has requested a withdrawal from the vault.
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(CAP),
    })
    .unwrap();
    let vault = guarded_vault_request(&mut s);
    // The breaker halts Module::Account: G cannot lift its own limit.
    let halt = BreakerRecord {
        module: Module::Account,
        invariant: Invariant::CouncilPause,
        tripped_at: s.chain.height,
        until: s.chain.height + 1_000,
    };
    s.chain
        .db
        .raw_put_for_test(&guard_key(Module::Account), &halt.encode())
        .unwrap();
    let lift = s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::zero(),
    });
    assert!(lift.unwrap_err().contains("halted"), "the lift is halted");
    // The cancel, never halted, must not be refused by G's window either.
    s.guardian_cancels(vault)
        .expect("a guardian's cancel is open to a limited account");
    assert_eq!(s.chain.balance(&vault), U256::from(FUNDS));
}

#[test]
fn an_owner_revokes_a_leaked_session_key_during_a_halt() {
    use custom_l1_node::state::invariant_guard::{BreakerRecord, Invariant, Module, guard_key};
    // Decision 8: a halt of Module::Account must not leave a leaked key live.
    let mut s = Scene::opted_in();
    let halt = BreakerRecord {
        module: Module::Account,
        invariant: Invariant::CouncilPause,
        tripped_at: s.chain.height,
        until: s.chain.height + 1_000,
    };
    s.chain
        .db
        .raw_put_for_test(&guard_key(Module::Account), &halt.encode())
        .unwrap();
    let digest = s.session_policy.digest();
    s.owner_does(AccountAction::RevokeSessionKey { key_hash: digest })
        .expect("revoke is never halted");
    refused(&mut s, "authority moved", |s| s.session_pays(PAYEE, 1));
}

#[test]
fn a_sixteen_recipient_scope_pays_its_last_recipient() {
    // The membership test's bound (MAX_SESSION_RECIPIENTS) is admitted in
    // full; seventeen does not decode (vector AA-seventeen-recipients).
    let mut s = Scene::opted_in();
    let recipients: Vec<Address> = (0..16u8).map(|i| [0xC0 + i; 32]).collect();
    s.owner_does(AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(CAP, EXPIRES, recipients.clone()),
    })
    .unwrap();
    s.session_pays(recipients[15], 1)
        .expect("the sixteenth recipient");
}

#[test]
fn a_session_key_at_its_cap_cannot_burn_the_account_in_fees() {
    // PR #548 review: with the required fee outside the cap, a leaked key at
    // its cap could still sign frames that moved nothing but the fee, burning
    // the balance until expiry and taking every nonce so the owner's revoke
    // went stale. The cap now counts every output, the fee included, and a
    // frame must pay a listed recipient something.
    let mut s = Scene::opted_in();
    let required = required_fee(|fee| s.session_pays_with_fee(1, fee));
    s.regrant_session(1 + required).unwrap();
    s.fees_on();
    let tx = s.session_pays_with_fee(1, required);
    s.chain
        .send(tx)
        .expect("1 plus the required fee == max_amount");
    let session_sends = |s: &mut Scene, outputs| {
        let tx = s.as_session(TxKind::Transfer, outputs);
        s.chain.send(tx)
    };
    refused(&mut s, "listed recipient", |s| {
        session_sends(s, with_fee(vec![], required))
    });
    refused(&mut s, "listed recipient", |s| {
        session_sends(s, with_fee(pay_to(PAYEE, 0), required))
    });
    refused(&mut s, "max_amount", |s| {
        let tx = s.session_pays_with_fee(1, required);
        s.chain.send(tx)
    });
    // The key holds no nonce, so the owner's revoke at the next nonce lands.
    let revoke = AccountAction::RevokeSessionKey {
        key_hash: s.session_policy.digest(),
    };
    let fee = required_fee(|fee| s.as_owner(account(revoke.clone()), with_fee(vec![], fee)));
    let tx = s.as_owner(account(revoke), with_fee(vec![], fee));
    s.chain.send(tx).expect("the owner revokes");
}

#[test]
fn a_session_frame_paying_nobody_cannot_take_nonces_with_fees_off() {
    // Without fees a zero-value frame would cost the key nothing and take a
    // nonce each time: the same stale-revoke attack, for free.
    let mut s = Scene::opted_in();
    refused(&mut s, "listed recipient", |s| s.session_pays(PAYEE, 0));
    refused(&mut s, "listed recipient", |s| {
        let tx = s.as_session(TxKind::Transfer, vec![]);
        s.chain.send(tx)
    });
}
