//! Smart-account policy (ADR-135 P-5, feature bit 45): session keys with a
//! cap, an expiry and a recipient list, and a per-window spending limit, on
//! account authority's v9 frame. Dark: every test sets both activation
//! heights; the compiled ones are `u64::MAX`.
//!
//! Red-team cases (ADR-135 S-1, S-9, S-10 and this build's own): a session
//! key over its cap, past its expiry, to an unlisted recipient, signing an
//! account action, after revocation, after the authority that granted it
//! changed, over the verification budget, and straddling the window with an
//! owner-signed frame.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/account_scene.rs"]
mod account_scene;

use account_scene::*;

// ------------------------------------------------------------------- tests --

#[test]
fn a_session_key_pays_a_listed_recipient_up_to_its_cap() {
    let mut s = Scene::opted_in();
    s.session_pays(PAYEE, CAP).expect("amount == max_amount");
    assert_eq!(s.chain.balance(&PAYEE), U256::from(CAP));
    assert_eq!(s.chain.nonce(&s.address), 2);
}

#[test]
fn a_session_key_over_its_cap_is_refused() {
    let mut s = Scene::opted_in();
    refused(&mut s, "max_amount", |s| s.session_pays(PAYEE, CAP + 1));
    // Two outputs each under the cap: the cap is on the frame, not the output.
    let two = vec![
        TxOutput {
            amount: U256::from(CAP / 2 + 1),
            recipient: PAYEE,
        },
        TxOutput {
            amount: U256::from(CAP / 2),
            recipient: PAYEE,
        },
    ];
    refused(&mut s, "max_amount", |s| {
        let tx = s.as_session(TxKind::Transfer, two);
        s.chain.send(tx)
    });
}

#[test]
fn an_expired_session_key_is_refused() {
    let mut s = Scene::opted_in();
    s.chain.next_at(EXPIRES);
    s.session_pays(PAYEE, 1).expect("height == expires_at");
    refused(&mut s, "expired", |s| s.session_pays(PAYEE, 1));
}

#[test]
fn an_unlisted_recipient_is_refused() {
    let mut s = Scene::opted_in();
    refused(&mut s, "may not pay", |s| s.session_pays(STRANGER, 1));
}

#[test]
fn a_session_key_cannot_sign_account_actions() {
    let mut s = Scene::opted_in();
    // S-1: widening its own scope.
    let widen = AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(u64::MAX, EXPIRES, vec![PAYEE, STRANGER]),
    };
    refused(&mut s, "only plain transfers", |s| {
        let tx = s.as_session(account(widen), vec![]);
        s.chain.send(tx)
    });
    // Nor taking the account: a new policy, a lifted limit.
    let take = AccountAction::AdoptPolicy(s.session_policy.clone());
    refused(&mut s, "only plain transfers", |s| {
        let tx = s.as_session(account(take), vec![]);
        s.chain.send(tx)
    });
    refused(&mut s, "only plain transfers", |s| {
        let tx = s.as_session(
            account(AccountAction::SetDailyLimit {
                limit: U256::zero(),
            }),
            vec![],
        );
        s.chain.send(tx)
    });
}

#[test]
fn a_revoked_session_key_is_refused() {
    let mut s = Scene::opted_in();
    s.session_pays(PAYEE, 1).unwrap();
    let digest = s.session_policy.digest();
    s.owner_does(AccountAction::RevokeSessionKey { key_hash: digest })
        .expect("owner revokes");
    assert_eq!(
        s.chain
            .db
            .committed_account_policy(&s.address, &digest)
            .unwrap()
            .0,
        None
    );
    refused(&mut s, "authority moved", |s| s.session_pays(PAYEE, 1));
    // A second revoke has nothing to remove.
    let err = s
        .owner_does(AccountAction::RevokeSessionKey { key_hash: digest })
        .unwrap_err();
    assert!(err.contains("no such session key"), "{err}");
}

#[test]
fn an_authority_change_voids_every_session_key() {
    // A thief holding the owner key adds a session key for himself; the
    // account then moves to a new policy (here the owner's AdoptPolicy, with
    // no guardians, which is immediate; a finished recovery writes the record
    // the same way). The thief's session key must die with the old authority.
    let mut s = Scene::opted_in();
    let new_owner = device(0x0A);
    let new_policy = session_policy(&new_owner);
    s.owner_does(AccountAction::AdoptPolicy(new_policy.clone()))
        .expect("adopt");
    refused(&mut s, "authority moved", |s| s.session_pays(PAYEE, 1));
    // Re-granting under the new authority brings it back.
    let regrant = AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(CAP, EXPIRES, vec![PAYEE]),
    };
    let tx = v9_frame(
        s.address,
        &new_policy,
        account(regrant),
        vec![],
        s.chain.nonce(&s.address),
        |m| {
            vec![Approval {
                index: 0,
                signature: MlDsa65::sign(&new_owner, m).unwrap(),
            }]
        },
    );
    s.chain.send(tx).expect("regrant");
    s.session_pays(PAYEE, 1)
        .expect("session key under the new authority");
}

#[test]
fn a_voided_session_key_stays_void_when_the_old_authority_returns() {
    // A -> B -> A: the authority digest is A's again, but the session key
    // granted under the first A (perhaps leaked, the reason for the change)
    // must not count again. A key is bound to the authority generation.
    let mut s = Scene::opted_in();
    let other = device(0x0A);
    let b = session_policy(&other);
    s.owner_does(AccountAction::AdoptPolicy(b.clone()))
        .expect("A -> B");
    let back = v9_frame(
        s.address,
        &b,
        account(AccountAction::AdoptPolicy(s.owner_policy.clone())),
        vec![],
        s.chain.nonce(&s.address),
        |m| {
            vec![Approval {
                index: 0,
                signature: MlDsa65::sign(&other, m).unwrap(),
            }]
        },
    );
    s.chain.send(back).expect("B -> A");
    s.owner_does(AccountAction::SetDailyLimit { limit: U256::one() })
        .expect("A signs for the account again");
    refused(&mut s, "authority moved", |s| s.session_pays(PAYEE, 1));
}

#[test]
fn the_window_caps_owner_and_session_frames_together() {
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(150u64),
    })
    .unwrap();
    s.session_pays(PAYEE, CAP).unwrap();
    // S-9: an owner-signed frame counts against the same window.
    let owner_pay = |s: &mut Scene, amount| {
        let tx = s.as_owner(TxKind::Transfer, pay_to(STRANGER, amount));
        s.chain.send(tx)
    };
    owner_pay(&mut s, 50).expect("spent + amount == limit");
    refused(&mut s, "past the account's limit", |s| owner_pay(s, 1));
    refused(&mut s, "past the account's limit", |s| {
        s.session_pays(PAYEE, 1)
    });
    // The next window starts empty.
    s.chain.next_at(WINDOW_BLOCKS);
    owner_pay(&mut s, 150).expect("a new window");
    // Zero removes the cap.
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::zero(),
    })
    .unwrap();
    owner_pay(&mut s, 1_000).expect("no cap");
}

#[test]
fn with_fees_on_a_session_frame_pays_its_fee_outside_its_cap_and_recipients() {
    // The recipient list never names the fee collector and the required fee
    // is outside `max_amount`: without both, no session key could pay a fee.
    let mut s = Scene::opted_in();
    s.fees_on();
    let required = required_fee(|fee| s.session_pays_with_fee(CAP, fee));
    // A tip above the required fee is value the key moved: it counts.
    refused(&mut s, "max_amount", |s| {
        let tx = s.session_pays_with_fee(CAP, required + 1);
        s.chain.send(tx)
    });
    let tx = s.session_pays_with_fee(CAP, required);
    s.chain.send(tx).expect("CAP plus exactly the required fee");
    assert_eq!(s.chain.balance(&PAYEE), U256::from(CAP));
    assert_eq!(
        s.chain.balance(&s.address),
        U256::from(FUNDS - CAP - required)
    );
}

#[test]
fn with_fees_on_the_required_fee_is_outside_the_window() {
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(150u64),
    })
    .unwrap();
    s.fees_on();
    let required = required_fee(|fee| s.owner_pays(150, fee));
    let tx = s.owner_pays(150, required);
    s.chain
        .send(tx)
        .expect("the window filled to its limit, the fee exempt");
    refused(&mut s, "past the account's limit", |s| {
        let tx = s.owner_pays(1, required);
        s.chain.send(tx)
    });
}

#[test]
fn the_mempool_admits_a_session_frame_inside_its_scope() {
    let s = Scene::opted_in();
    s.fees_on();
    // The mempool judges at the tip under the committed schedule: a tip at
    // height 0 and a version-2 entry switching bits 9 and 45 on at height 1.
    s.chain
        .db
        .init_chain(
            &s.chain.block(vec![]),
            custom_l1_node::consensus::U256::ZERO,
        )
        .unwrap();
    let features = FeatureSet::ACCOUNT_AUTHORITY | FeatureSet::ACCOUNT_POLICY;
    // The version record: version u32 LE, activation height u64 LE, bits u64 LE.
    let entry = [
        2u32.to_le_bytes().as_slice(),
        1u64.to_le_bytes().as_slice(),
        features.to_le_bytes().as_slice(),
    ]
    .concat();
    s.chain
        .db
        .raw_put_for_test(&version_key(2), &entry)
        .unwrap();
    let required = required_fee(|fee| s.session_pays_with_fee(CAP, fee));
    let inside = s.session_pays_with_fee(CAP, required);
    let over = s.session_pays_with_fee(CAP + 1, required);
    let pool = Mempool::new(Arc::new(s.chain.db));
    pool.validate(&inside)
        .expect("a session frame inside its scope");
    let err = pool.validate(&over).unwrap_err().to_string();
    assert!(err.contains("max_amount"), "{err}");
}

#[test]
fn a_failed_transfer_leaves_no_window_charge_in_the_builder() {
    // The window is charged before the balance check. A transfer that then
    // fails must not leave its charge behind for the next frame to trip on.
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(2 * FUNDS),
    })
    .unwrap();
    let broke = s.as_owner(TxKind::Transfer, pay_to(STRANGER, 2 * FUNDS));
    let good = s.as_owner(TxKind::Transfer, pay_to(STRANGER, 1));
    let kept = s
        .chain
        .db
        .select_applicable(
            vec![broke, good.clone()],
            s.chain.context(s.chain.height + 1),
            target_from_leading_zero_bits(0),
        )
        .unwrap();
    assert_eq!(
        kept,
        vec![good],
        "the same-nonce transfer inside the window is kept"
    );
}

fn slh_key(seed: u8) -> PolicyKey {
    let key = SlhDsaSha2_128s::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]));
    PolicyKey {
        suite: SuiteId::SlhDsaSha2_128s,
        public_key: SlhDsaSha2_128s::public_key(&key),
    }
}

#[test]
fn session_approvals_over_the_cost_budget_are_refused_before_any_signature() {
    // Four SLH-DSA-128s keys (10 units each) and one ML-DSA-65 key (1 unit).
    let mut keys: Vec<PolicyKey> = (0x40..0x44).map(slh_key).collect();
    keys.push(PolicyKey {
        suite: SuiteId::MlDsa65,
        public_key: MlDsa65::public_key(&device(0x45)),
    });
    let wide = MultisigPolicy::new(1, keys).unwrap();
    let mut s = Scene::opted_in();
    let grant = AccountAction::AddSessionKey {
        key_hash: wide.digest(),
        scope: scope(CAP, EXPIRES, vec![PAYEE]),
    };
    s.owner_does(grant).unwrap();
    // Garbage signatures: the budget is checked before any is verified.
    let frame = |s: &Scene, n: u8| {
        v9_frame(
            s.address,
            &wide,
            TxKind::Transfer,
            pay_to(PAYEE, 1),
            s.chain.nonce(&s.address),
            |_| {
                (0..n)
                    .map(|index| Approval {
                        index,
                        signature: vec![0; 8],
                    })
                    .collect()
            },
        )
    };
    assert_eq!(MAX_SESSION_COST, 40);
    let over = frame(&s, 5); // 41 units
    refused(&mut s, "over the budget", |s| s.chain.send(over));
    let at = frame(&s, 4); // 40 units: past the budget, refused by the signature check
    let err = s.chain.send(at).unwrap_err();
    assert!(
        !err.contains("budget"),
        "40 units is inside the budget: {err}"
    );
}

#[test]
fn session_actions_are_refused_before_bit_45() {
    let mut s = Scene::with_policy_activation(1_000);
    let err = s.opt_in_with_session().unwrap_err();
    assert!(err.contains("session keys are not active"), "{err}");
    let pool = Mempool::new(Arc::new(s.chain.db));
    let action = AccountAction::SetDailyLimit { limit: U256::one() };
    let mut tx = Transaction::with_kind(account(action), 0);
    tx.sign(&s.owner, &CHAIN).unwrap();
    let err = pool.validate(&tx).unwrap_err().to_string();
    assert!(err.contains("not active"), "{err}");
}

#[test]
fn a_session_shaped_frame_is_refused_before_bit_45() {
    // Account authority on, policy dark: a v9 frame under a non-authority
    // policy is refused exactly as before this feature.
    let mut s = Scene::with_policy_activation(1_000);
    // Opt in through account authority alone (an AdoptPolicy of itself).
    let mut tx = Transaction::with_kind(
        account(AccountAction::AdoptPolicy(s.owner_policy.clone())),
        0,
    );
    tx.sign(&s.owner, &CHAIN).unwrap();
    s.chain.send(tx).expect("authority opt-in");
    refused(&mut s, "not the one signing", |s| s.session_pays(PAYEE, 1));
}

#[test]
fn session_and_window_rows_are_undone_by_a_reorg() {
    let mut s = Scene::new();
    let before = s.chain.db.state_root().unwrap();
    let action = AccountAction::AddSessionKey {
        key_hash: s.session_policy.digest(),
        scope: scope(CAP, EXPIRES, vec![PAYEE]),
    };
    let mut tx = Transaction::with_kind(account(action), 0);
    tx.sign(&s.owner, &CHAIN).unwrap();
    s.chain.height = 1;
    let mut block = s.chain.block(vec![tx]);
    let context = s.chain.context(1);
    block.header.state_root = s.chain.db.preview_root(&block, context).unwrap();
    let after = s
        .chain
        .db
        .apply_block_journaled(&block, &[1; 32], context)
        .unwrap();
    assert_ne!(after, before);
    assert_eq!(s.chain.db.uncovered_keys().unwrap(), Vec::<Vec<u8>>::new());
    s.chain.db.revert_block(&[1; 32]).unwrap();
    let digest = s.session_policy.digest();
    assert_eq!(
        s.chain
            .db
            .committed_account_policy(&s.address, &digest)
            .unwrap(),
        (None, None)
    );
    assert_eq!(s.chain.db.state_root().unwrap(), before);
}

fn transfer_roots(policy: u64) -> Vec<[u8; 32]> {
    let owner = signing_key_from_seed(&[0x04; 32]).unwrap();
    let mut chain = Chain::new(&[owner.address()], 1, policy);
    let mut roots = vec![chain.db.state_root().unwrap()];
    for amount in [3, 4] {
        let mut tx = Transaction::with_kind(TxKind::Transfer, chain.nonce(&owner.address()));
        tx.outputs = pay_to(PAYEE, amount);
        tx.sign(&owner, &CHAIN).unwrap();
        chain.send(tx).unwrap();
        roots.push(chain.db.state_root().unwrap());
    }
    roots
}

#[test]
fn the_root_is_unchanged_when_no_session_key_or_limit_exists() {
    assert_eq!(transfer_roots(1), transfer_roots(u64::MAX));
}

// ----------------------------------------------------- conformance vectors --

const VECTORS_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spec/tests/account-policy.json"
);

fn outcome(r: Result<(), String>) -> serde_json::Value {
    match r {
        Ok(()) => serde_json::json!({"result": "ok"}),
        Err(e) if e.contains("authority moved") => {
            serde_json::json!({"result": "error", "error": "AuthorityMoved"})
        }
        Err(e) if e.contains("account authority") => {
            serde_json::json!({"result": "error", "error": "Account"})
        }
        Err(e) => serde_json::json!({"result": "error", "error": e}),
    }
}

fn kind_hex(action: AccountAction) -> String {
    let mut out = Vec::new();
    account(action).encode_into(&mut out);
    hex::encode(out)
}

#[allow(clippy::too_many_lines)]
fn vector_cases() -> serde_json::Value {
    use custom_l1_node::core::codec::ByteReader;
    let add = AccountAction::AddSessionKey {
        key_hash: [0x11; 32],
        scope: scope(CAP, EXPIRES, vec![PAYEE]),
    };
    let mut seventeen = hex::decode(kind_hex(AccountAction::AddSessionKey {
        key_hash: [0x11; 32],
        scope: scope(CAP, EXPIRES, vec![]),
    }))
    .unwrap();
    seventeen.truncate(seventeen.len() - 4);
    seventeen.extend_from_slice(&17u32.to_le_bytes());
    seventeen.extend_from_slice(&[0x22; 32 * 17]);
    // The rule (n <= 16 at decode), checked here so a regenerate cannot
    // write whatever the decoder happens to do into the spec.
    assert!(
        TxKind::decode(&mut ByteReader::new(&seventeen)).is_err(),
        "17 session recipients must not decode"
    );

    let at_cap = Scene::opted_in().session_pays(PAYEE, CAP);
    let over_cap = Scene::opted_in().session_pays(PAYEE, CAP + 1);
    let mut s = Scene::opted_in();
    s.chain.next_at(EXPIRES);
    let at_expiry = s.session_pays(PAYEE, 1);
    let past_expiry = s.session_pays(PAYEE, 1);
    let unlisted = Scene::opted_in().session_pays(STRANGER, 1);
    let mut s = Scene::opted_in();
    let tx = s.as_session(
        account(AccountAction::SetDailyLimit {
            limit: U256::zero(),
        }),
        vec![],
    );
    let session_sub_op = s.chain.send(tx);
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::RevokeSessionKey {
        key_hash: s.session_policy.digest(),
    })
    .unwrap();
    let revoked = s.session_pays(PAYEE, 1);
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(CAP + 1),
    })
    .unwrap();
    let window_fill = s.session_pays(PAYEE, CAP);
    let tx = s.as_owner(TxKind::Transfer, pay_to(STRANGER, 2));
    let window_over = s.chain.send(tx);
    let dark = Scene::with_policy_activation(1_000).opt_in_with_session();
    let mut s = Scene::opted_in();
    s.owner_does(AccountAction::SetDailyLimit {
        limit: U256::from(CAP),
    })
    .unwrap();
    let escrow = s.as_owner(
        TxKind::OpenChannel(custom_l1_node::core::ChannelOpen {
            counterparty: STRANGER,
            funding: U256::from(FUNDS / 2),
            dispute_window: 50,
        }),
        vec![],
    );
    let window_escrow = s.chain.send(escrow);

    serde_json::json!({
        "description": "ADR-135 P-5 smart-account policy: session-key action encodings, session-frame scope, the spending window and activation. Regenerate with MAYA_REGEN_VECTORS=1 cargo test -p custom-l1-node --test account_policy_tests conformance; every implementation must agree.",
        "cases": [
            {"id": "AA-add-session-key-bytes", "rules": ["AA-7"],
             "input": {"key_hash": hex::encode([0x11; 32]), "max_amount": CAP.to_string(), "expires_at": EXPIRES, "recipients": [hex::encode(PAYEE)]},
             "expect": {"result": "ok", "kind_bytes": kind_hex(add)}},
            {"id": "AA-revoke-session-key-bytes", "rules": ["AA-7"],
             "expect": {"result": "ok", "kind_bytes": kind_hex(AccountAction::RevokeSessionKey { key_hash: [0x11; 32] })}},
            {"id": "AA-set-daily-limit-bytes", "rules": ["AA-7"],
             "expect": {"result": "ok", "kind_bytes": kind_hex(AccountAction::SetDailyLimit { limit: U256::from(150u64) })}},
            {"id": "AA-seventeen-recipients", "rules": ["AA-7"],
             "input": {"kind_bytes": hex::encode(&seventeen)},
             "expect": {"result": "error", "error": "Decode"}},
            {"id": "AA-session-at-cap", "rules": ["AA-8"], "input": {"amount": CAP.to_string(), "max_amount": CAP.to_string()}, "expect": outcome(at_cap)},
            {"id": "AA-session-over-cap", "rules": ["AA-8"], "input": {"amount": (CAP + 1).to_string(), "max_amount": CAP.to_string()}, "expect": outcome(over_cap)},
            {"id": "AA-session-at-expiry", "rules": ["AA-8"], "input": {"height": EXPIRES, "expires_at": EXPIRES}, "expect": outcome(at_expiry)},
            {"id": "AA-session-past-expiry", "rules": ["AA-8"], "input": {"height": EXPIRES + 1, "expires_at": EXPIRES}, "expect": outcome(past_expiry)},
            {"id": "AA-session-unlisted-recipient", "rules": ["AA-8"], "expect": outcome(unlisted)},
            {"id": "AA-session-signed-sub-op", "rules": ["AA-8"], "expect": outcome(session_sub_op)},
            {"id": "AA-session-revoked", "rules": ["AA-8"], "expect": outcome(revoked)},
            {"id": "AA-window-at-limit", "rules": ["AA-9"], "input": {"limit": (CAP + 1).to_string(), "spent": "0", "amount": CAP.to_string()}, "expect": outcome(window_fill)},
            {"id": "AA-window-owner-over-limit", "rules": ["AA-9"], "input": {"limit": (CAP + 1).to_string(), "spent": CAP.to_string(), "amount": "2"}, "expect": outcome(window_over)},
            {"id": "AA-window-refuses-a-kind-without-outputs", "rules": ["AA-9"], "input": {"limit": CAP.to_string(), "kind": "open_channel", "funding": (FUNDS / 2).to_string()}, "expect": outcome(window_escrow)},
            {"id": "AA-policy-before-bit-45", "rules": ["AA-10"], "expect": outcome(dark)},
            {"id": "AA-root-without-policy-rows", "rules": ["AA-10"], "expect": {"result": "ok", "roots_equal_dark_chain": transfer_roots(1) == transfer_roots(u64::MAX)}}
        ]
    })
}

#[test]
fn conformance_vectors_match_the_spec_file() {
    let computed = vector_cases();
    if std::env::var_os("MAYA_REGEN_VECTORS").is_some() {
        let text = serde_json::to_string_pretty(&computed).unwrap() + "\n";
        std::fs::write(VECTORS_PATH, text).unwrap();
    }
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(VECTORS_PATH).unwrap()).unwrap();
    assert_eq!(stored, computed, "spec/tests/account-policy.json is stale");
}
