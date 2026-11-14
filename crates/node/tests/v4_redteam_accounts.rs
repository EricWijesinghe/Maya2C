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
