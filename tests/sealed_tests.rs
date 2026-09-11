//! The sealed mempool against real block execution.
//!
//! `mev/tests/` proves the threshold cryptography. These cover everything the
//! cryptography deliberately does not know about: when an envelope may be
//! opened, who may contribute to opening it, what happens when nobody does,
//! and — the one that matters most — whether a revealed action that fails can
//! be used to void a block.
//!
//! Four of these are load-bearing rather than merely nice:
//!
//! - **An envelope cannot be opened in the block that included it.** That
//!   single inequality is the whole scheme; everything else is arithmetic.
//! - **A share is valid only in the reveal block.** Otherwise the plaintext
//!   appears while blocks are still being built against the ciphertext, and the
//!   advantage moves from the miner to the committee rather than going away.
//! - **A revealed action that fails must not fail the block.** A failing
//!   transaction fails its whole block on this chain, so the alternative hands
//!   anyone a censorship weapon costing one fee, aimed at a block chosen days
//!   in advance.
//! - **An expired envelope must move nothing.** A committee that goes dark
//!   must cost liveness, never safety.

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::dex_payload::{AssetRegistration, PoolCreation, SwapRequest};
use custom_l1_node::core::governance_payload::StakeLock;
use custom_l1_node::core::sealed_payload::{RevealShare, SealedEnvelope};
use custom_l1_node::core::{
    Block, BlockHeader, Transaction, TxKind, derive_envelope_id, envelope_aad,
};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::sealed::{
    CommitteeRecord, MAX_REVEAL_DELAY, MIN_REVEAL_DELAY, envelope_key, share_key,
};
use custom_l1_node::state::{
    Account, Address, BlockContext, Direction, NATIVE_ASSET, StateDB, derive_asset_id,
    derive_pair_id,
};
use maya_mev::committee::{Committee, MemberSecret};
use maya_mev::seal;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// Three of five: a committee that survives two absentees and needs no more
/// than a majority to act.
const THRESHOLD: u16 = 3;
const MEMBERS: u16 = 5;

struct Fixture {
    db: StateDB,
    committee: Committee,
    secrets: Vec<MemberSecret>,
    _dir: TempDir,
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
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

/// A chain with a seeded committee and the given funded accounts.
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

    let (committee, secrets) = Committee::generate(THRESHOLD, MEMBERS).expect("committee");
    db.seed_sealed_committee(&CommitteeRecord::from_committee(&committee))
        .expect("seed");

    Fixture {
        db,
        committee,
        secrets,
        _dir: dir,
    }
}

impl Fixture {
    /// Wraps `kind` in an envelope from `sender`, opening at `reveal_height`.
    fn seal(
        &self,
        kind: &TxKind,
        sender: &HybridSigningKey,
        nonce: u64,
        reveal_height: u64,
    ) -> (Transaction, [u8; 32]) {
        let id = derive_envelope_id(&sender.address(), nonce);

        let mut plaintext = Vec::new();
        kind.encode_into(&mut plaintext);
        let payload = seal(
            &self.committee,
            &plaintext,
            &envelope_aad(&id, reveal_height),
        )
        .expect("seal");

        let envelope = SealedEnvelope {
            reveal_height,
            ciphertext: payload.encode(),
        };
        (signed(TxKind::Seal(Box::new(envelope)), nonce, sender), id)
    }

    /// Reveal-share transactions from the first `count` committee members.
    ///
    /// Each is a real transaction from a real account, because a committee
    /// member is an ordinary chain participant here — the committee is a
    /// cryptographic role, not a privileged transaction path.
    fn shares(
        &self,
        id: &[u8; 32],
        reveal_height: u64,
        count: usize,
        keepers: &[(HybridSigningKey, u64)],
    ) -> Vec<Transaction> {
        let ciphertext = self.stored_ciphertext(id, reveal_height);
        let payload = maya_mev::SealedPayload::decode(&ciphertext).expect("decode");

        self.secrets
            .iter()
            .take(count)
            .zip(keepers)
            .map(|(secret, (key, nonce))| {
                let share = secret.decryption_share(&payload).expect("share");
                signed(
                    TxKind::RevealShare(RevealShare {
                        envelope: *id,
                        member: share.index,
                        share: share.share.to_bytes(),
                        proof: share.proof.encode(),
                    }),
                    *nonce,
                    key,
                )
            })
            .collect()
    }

    /// The ciphertext chain state holds for a pending envelope.
    fn stored_ciphertext(&self, id: &[u8; 32], reveal_height: u64) -> Vec<u8> {
        self.db
            .pending_envelope(reveal_height, id)
            .expect("read")
            .expect("envelope is pending")
            .ciphertext
    }

    fn is_pending(&self, id: &[u8; 32], reveal_height: u64) -> bool {
        self.db
            .pending_envelope(reveal_height, id)
            .expect("read")
            .is_some()
    }

    fn balance(&self, address: &Address) -> u64 {
        self.db.get_account(address).expect("account").balance
    }

    fn apply(&self, transactions: Vec<Transaction>, height: u64) -> Result<(), NodeError> {
        self.db
            .apply_block(&block_of(transactions), BlockContext::at_height(height))
            .map(|_| ())
    }
}

/// Two committee members' own accounts, funded so they can pay for shares.
fn keepers(count: usize) -> Vec<(HybridSigningKey, u64)> {
    (0..count)
        .map(|_| (generate_signing_key().expect("key"), 0u64))
        .collect()
}

// ---------------------------------------------------------------------------
// the lifecycle
// ---------------------------------------------------------------------------

#[test]
fn an_envelope_executes_at_its_reveal_height_and_not_before() {
    let sender = generate_signing_key().expect("key");
    let address = sender.address();
    let keepers = keepers(3);

    let mut funded = vec![(address, 1_000_000u64)];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    let locked = 250_000;
    let (envelope, id) = fixture.seal(
        &TxKind::LockStake(StakeLock {
            amount: locked,
            unlock_height: 100_000,
        }),
        &sender,
        0,
        5,
    );

    fixture.apply(vec![envelope], 1).expect("include");
    // Included and ordered, and the action has not happened: the balance is
    // untouched, and nothing on chain says what the envelope contains.
    assert_eq!(fixture.balance(&address), 1_000_000);
    assert!(fixture.is_pending(&id, 5));

    // Nothing happens in the intervening blocks either.
    fixture.apply(vec![], 2).expect("quiet block");
    fixture.apply(vec![], 3).expect("quiet block");
    assert_eq!(fixture.balance(&address), 1_000_000);

    fixture
        .apply(fixture.shares(&id, 5, 3, &keepers), 5)
        .expect("reveal");

    assert_eq!(fixture.balance(&address), 1_000_000 - locked);
    assert_eq!(
        fixture.db.stake_lock(&address).expect("lock").amount,
        locked
    );
    assert!(
        !fixture.is_pending(&id, 5),
        "an opened envelope must be consumed, or it could be opened again"
    );
}

#[test]
fn an_envelope_nobody_opens_expires_and_moves_nothing() {
    let sender = generate_signing_key().expect("key");
    let address = sender.address();
    let fixture = fixture(&[(address, 1_000_000)]);

    let (envelope, id) = fixture.seal(
        &TxKind::LockStake(StakeLock {
            amount: 250_000,
            unlock_height: 100_000,
        }),
        &sender,
        0,
        4,
    );
    fixture.apply(vec![envelope], 1).expect("include");

    // The reveal block arrives with no shares in it. A committee that has gone
    // dark must cost liveness and never safety, so this is a block like any
    // other rather than an error.
    fixture.apply(vec![], 4).expect("reveal block");

    assert_eq!(fixture.balance(&address), 1_000_000);
    assert_eq!(fixture.db.stake_lock(&address).expect("lock").amount, 0);
    assert!(
        !fixture.is_pending(&id, 4),
        "an expired envelope must be consumed, or it sits in state forever"
    );
}

#[test]
fn one_share_short_of_the_threshold_opens_nothing() {
    let sender = generate_signing_key().expect("key");
    let address = sender.address();
    let keepers = keepers(2);

    let mut funded = vec![(address, 1_000_000u64)];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    let (envelope, id) = fixture.seal(
        &TxKind::LockStake(StakeLock {
            amount: 250_000,
            unlock_height: 100_000,
        }),
        &sender,
        0,
        4,
    );
    fixture.apply(vec![envelope], 1).expect("include");

    // Two of three. The shares are valid, they are accepted onto the chain, and
    // the envelope still expires — a threshold is a threshold.
    fixture
        .apply(fixture.shares(&id, 4, 2, &keepers), 4)
        .expect("reveal block");

    assert_eq!(fixture.balance(&address), 1_000_000);
    assert!(!fixture.is_pending(&id, 4));
}

// ---------------------------------------------------------------------------
// the inequality the scheme rests on
// ---------------------------------------------------------------------------

#[test]
fn an_envelope_cannot_be_opened_in_the_block_that_included_it() {
    let sender = generate_signing_key().expect("key");
    let fixture = fixture(&[(sender.address(), 1_000_000)]);

    // A reveal height equal to the including height would let the miner who
    // chose this block's contents read what they were ordering. There is no
    // configuration in which that is permitted.
    let (envelope, _) = fixture.seal(&TxKind::Transfer, &sender, 0, 1);

    let error = fixture.apply(vec![envelope], 1).expect_err("must refuse");
    assert!(
        matches!(error, NodeError::SealedRevealWindow { min, .. } if min == MIN_REVEAL_DELAY),
        "{error}"
    );
}

#[test]
fn an_envelope_cannot_be_opened_in_the_next_block_either() {
    let sender = generate_signing_key().expect("key");
    let fixture = fixture(&[(sender.address(), 1_000_000)]);

    // A delay of one leaves the committee no slack at all: it would have to see
    // the envelope in a committed block and land its shares in the very next
    // one. That is a race the committee usually loses, not a threshold scheme.
    let (envelope, _) = fixture.seal(&TxKind::Transfer, &sender, 0, 2);

    assert!(matches!(
        fixture.apply(vec![envelope], 1),
        Err(NodeError::SealedRevealWindow { .. })
    ));
}

#[test]
fn an_envelope_cannot_wait_forever() {
    let sender = generate_signing_key().expect("key");
    let fixture = fixture(&[(sender.address(), 1_000_000)]);

    let (too_far, _) = fixture.seal(&TxKind::Transfer, &sender, 0, 1 + MAX_REVEAL_DELAY + 1);
    assert!(matches!(
        fixture.apply(vec![too_far], 1),
        Err(NodeError::SealedRevealWindow { .. })
    ));

    // The boundary itself is legal. A ceiling that is off by one is a ceiling
    // nobody can describe.
    let (at_limit, _) = fixture.seal(&TxKind::Transfer, &sender, 0, 1 + MAX_REVEAL_DELAY);
    fixture
        .apply(vec![at_limit], 1)
        .expect("the limit is legal");
}

#[test]
fn a_share_submitted_before_the_reveal_height_is_refused() {
    let sender = generate_signing_key().expect("key");
    let keepers = keepers(3);

    let mut funded = vec![(sender.address(), 1_000_000u64)];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    let (envelope, id) = fixture.seal(&TxKind::Transfer, &sender, 0, 6);
    fixture.apply(vec![envelope], 1).expect("include");

    // The shares are cryptographically perfect. They are refused because of
    // *when* they arrived: a plaintext published at height 3 would be a
    // plaintext that blocks 4 and 5 could be written against, which moves the
    // advantage from the miner to whoever collects shares fastest.
    let early = fixture.shares(&id, 6, 3, &keepers);
    let error = fixture.apply(early, 3).expect_err("must refuse");

    assert!(
        matches!(error, NodeError::UnknownSealedEnvelope(_)),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// what the committee can and cannot do
// ---------------------------------------------------------------------------

#[test]
fn a_share_that_fails_its_proof_is_refused_and_attributed() {
    let sender = generate_signing_key().expect("key");
    let keepers = keepers(3);

    let mut funded = vec![(sender.address(), 1_000_000u64)];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    let (envelope, id) = fixture.seal(&TxKind::Transfer, &sender, 0, 4);
    fixture.apply(vec![envelope], 1).expect("include");

    // Member 1 submits member 2's share under member 1's index. The point is a
    // perfectly good curve point; only the proof says otherwise.
    let mut shares = fixture.shares(&id, 4, 2, &keepers);
    let (TxKind::RevealShare(first), TxKind::RevealShare(second)) =
        (&shares[0].kind, &shares[1].kind)
    else {
        panic!("expected reveal shares");
    };
    let forged = RevealShare {
        member: first.member,
        ..*second
    };
    shares[0] = signed(TxKind::RevealShare(forged), 0, &keepers[0].0);

    let error = fixture.apply(shares, 4).expect_err("must refuse");
    assert!(
        matches!(error, NodeError::InvalidDecryptionShare { member, .. } if member == 1),
        "{error}"
    );
}

#[test]
fn a_share_from_a_non_member_is_refused() {
    let sender = generate_signing_key().expect("key");
    let keeper = generate_signing_key().expect("key");
    let fixture = fixture(&[(sender.address(), 1_000_000), (keeper.address(), 10_000)]);

    let (envelope, id) = fixture.seal(&TxKind::Transfer, &sender, 0, 4);
    fixture.apply(vec![envelope], 1).expect("include");

    let outsider = TxKind::RevealShare(RevealShare {
        envelope: id,
        member: MEMBERS + 1,
        share: [0u8; 32],
        proof: [0u8; 64],
    });

    assert!(matches!(
        fixture.apply(vec![signed(outsider, 0, &keeper)], 4),
        Err(NodeError::InvalidDecryptionShare { .. })
    ));
}

#[test]
fn a_member_cannot_submit_two_shares_for_one_envelope() {
    let sender = generate_signing_key().expect("key");
    let keepers = keepers(1);

    let fixture = fixture(&[
        (sender.address(), 1_000_000),
        (keepers[0].0.address(), 10_000),
    ]);

    let (envelope, id) = fixture.seal(&TxKind::Transfer, &sender, 0, 4);
    fixture.apply(vec![envelope], 1).expect("include");

    let first = fixture.shares(&id, 4, 1, &keepers);
    let mut again = fixture.shares(&id, 4, 1, &keepers);
    again[0] = signed(again[0].kind.clone(), 1, &keepers[0].0);

    let mut both = first;
    both.extend(again);

    // Interpolation divides by the difference of two member indices, so a
    // duplicate is a division by zero before it is a member voting twice.
    let error = fixture.apply(both, 4).expect_err("must refuse");
    assert!(
        matches!(error, NodeError::DuplicateDecryptionShare { member, .. } if member == 1),
        "{error}"
    );
}

#[test]
fn a_share_for_an_envelope_that_does_not_exist_is_refused() {
    let keeper = generate_signing_key().expect("key");
    let fixture = fixture(&[(keeper.address(), 10_000)]);

    let share = TxKind::RevealShare(RevealShare {
        envelope: [7u8; 32],
        member: 1,
        share: [0u8; 32],
        proof: [0u8; 64],
    });

    assert!(matches!(
        fixture.apply(vec![signed(share, 0, &keeper)], 4),
        Err(NodeError::UnknownSealedEnvelope(_))
    ));
}

// ---------------------------------------------------------------------------
// a revealed action that fails
// ---------------------------------------------------------------------------

#[test]
fn a_revealed_action_that_fails_does_not_fail_its_block() {
    let sender = generate_signing_key().expect("key");
    let address = sender.address();
    let bystander = generate_signing_key().expect("key");
    let keepers = keepers(3);

    let mut funded = vec![(address, 1_000), (bystander.address(), 500_000u64)];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    // A lock the sender cannot possibly afford. Sealed today, guaranteed to
    // fail at a height chosen days in advance — which is exactly the shape of
    // the censorship weapon that exists if a revealed failure aborts a block.
    let (envelope, id) = fixture.seal(
        &TxKind::LockStake(StakeLock {
            amount: u64::MAX,
            unlock_height: 100_000,
        }),
        &sender,
        0,
        4,
    );
    fixture.apply(vec![envelope], 1).expect("include");

    let mut reveal = fixture.shares(&id, 4, 3, &keepers);
    // An unrelated transaction in the same block. If the revealed failure
    // aborted the block, this would be reverted along with it — one sealed
    // transaction censoring an arbitrary stranger.
    let payment = {
        let mut tx = Transaction::with_kind(TxKind::Transfer, 0);
        tx.outputs.push(custom_l1_node::core::TxOutput {
            recipient: address,
            amount: 7_000,
        });
        tx.sign(&bystander).expect("sign");
        tx
    };
    reveal.push(payment);

    fixture.apply(reveal, 4).expect("the block must stand");

    // The stranger's payment landed; the sealed action did not happen.
    assert_eq!(fixture.balance(&address), 1_000 + 7_000);
    assert_eq!(fixture.db.stake_lock(&address).expect("lock").amount, 0);
    assert!(!fixture.is_pending(&id, 4));
}

#[test]
fn a_sealed_envelope_cannot_contain_another_one() {
    let sender = generate_signing_key().expect("key");
    let address = sender.address();
    let keepers = keepers(3);

    let mut funded = vec![(address, 1_000_000u64)];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    let inner = TxKind::Seal(Box::new(SealedEnvelope {
        reveal_height: 100,
        ciphertext: vec![0u8; 64],
    }));
    let (envelope, id) = fixture.seal(&inner, &sender, 0, 4);
    fixture.apply(vec![envelope], 1).expect("include");

    // The block stands and the nesting is discarded. A chain of envelopes would
    // be a queue that grows every time it is drained.
    fixture
        .apply(fixture.shares(&id, 4, 3, &keepers), 4)
        .expect("reveal");

    assert!(!fixture.is_pending(&id, 4));
    assert!(
        !fixture.is_pending(&id, 100),
        "a nested envelope must not be queued"
    );
}

// ---------------------------------------------------------------------------
// the MEV claim itself
// ---------------------------------------------------------------------------

#[test]
fn a_revealed_swap_clears_in_the_same_batch_as_a_plaintext_one() {
    // The heart of the design. A miner who learns a revealed swap while
    // assembling the reveal block can race it with a plaintext swap of their
    // own — and gains nothing, because both settle at the block's single
    // clearing price. See `docs/dex.md` on uniform-price batch clearing.
    let maker = generate_signing_key().expect("key");
    let victim = generate_signing_key().expect("key");
    let miner = generate_signing_key().expect("key");
    let keepers = keepers(3);

    let maker_address = maker.address();
    let mut funded = vec![
        (maker_address, 1_000_000u64),
        (victim.address(), 200_000u64),
        (miner.address(), 200_000u64),
    ];
    funded.extend(keepers.iter().map(|(key, _)| (key.address(), 10_000u64)));
    let fixture = fixture(&funded);

    let asset = derive_asset_id(&maker_address, 0, &symbol("USD"));
    let pair = derive_pair_id(&NATIVE_ASSET, &asset, 30);

    fixture
        .apply(
            vec![
                signed(
                    TxKind::RegisterAsset(AssetRegistration {
                        symbol: symbol("USD"),
                        total_supply: 4_000_000,
                    }),
                    0,
                    &maker,
                ),
                signed(
                    TxKind::CreatePool(PoolCreation {
                        asset_a: NATIVE_ASSET,
                        asset_b: asset,
                        lp_fee_bps: 30,
                        amount_a: 100_000,
                        amount_b: 400_000,
                    }),
                    1,
                    &maker,
                ),
            ],
            1,
        )
        .expect("market");

    let swap = |amount: u64, deadline: u64| {
        TxKind::Swap(SwapRequest {
            pair,
            direction: Direction::BaseToQuote.tag(),
            amount_in: amount,
            min_out: 0,
            deadline,
        })
    };

    let (envelope, id) = fixture.seal(&swap(20_000, 10), &victim, 0, 4);
    fixture.apply(vec![envelope], 2).expect("include");

    let mut reveal = fixture.shares(&id, 4, 3, &keepers);
    // The miner races the swap they can now read. It executes *before* the
    // revealed one, which is the only ordering they can achieve.
    reveal.push(signed(swap(20_000, 10), 0, &miner));
    fixture.apply(reveal, 4).expect("reveal block");

    let victim_out = fixture
        .db
        .asset_balance(&asset, &victim.address())
        .expect("balance");
    let miner_out = fixture
        .db
        .asset_balance(&asset, &miner.address())
        .expect("balance");

    assert!(victim_out > 0, "the sealed swap must have executed");
    assert_eq!(
        victim_out, miner_out,
        "two equal swaps in one block must settle at one price, whatever their order"
    );
}

fn symbol(text: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    out[..text.len()].copy_from_slice(text.as_bytes());
    out
}

// ---------------------------------------------------------------------------
// configuration
// ---------------------------------------------------------------------------

#[test]
fn a_chain_without_a_committee_refuses_sealed_transactions() {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open");
    let sender = generate_signing_key().expect("key");
    db.put_account(
        &sender.address(),
        &Account {
            balance: 1_000_000,
            nonce: 0,
        },
    )
    .expect("fund");

    let envelope = TxKind::Seal(Box::new(SealedEnvelope {
        reveal_height: 5,
        ciphertext: vec![0u8; 64],
    }));

    // Not "the sender did something wrong" — the feature is absent, and saying
    // so is the honest answer.
    let error = db
        .apply_block(
            &block_of(vec![signed(envelope, 0, &sender)]),
            BlockContext::at_height(1),
        )
        .expect_err("must refuse");
    assert!(matches!(error, NodeError::SealedCommitteeUnset), "{error}");
}

#[test]
fn a_committee_record_survives_the_wire() {
    let (committee, _) = Committee::generate(THRESHOLD, MEMBERS).expect("committee");
    let record = CommitteeRecord::from_committee(&committee);

    let decoded = CommitteeRecord::decode(&record.encode()).expect("decode");

    assert_eq!(decoded, record);
    assert_eq!(decoded.to_committee(), committee);
    decoded.validate().expect("a generated committee is valid");
}

#[test]
fn a_committee_that_cannot_be_met_is_refused_at_configuration() {
    let (committee, _) = Committee::generate(THRESHOLD, MEMBERS).expect("committee");

    let mut impossible = CommitteeRecord::from_committee(&committee);
    impossible.threshold = MEMBERS + 1;
    assert!(matches!(
        impossible.validate(),
        Err(NodeError::SealedCommittee { .. })
    ));

    let mut zero_index = CommitteeRecord::from_committee(&committee);
    zero_index.members[0].0 = 0;
    assert!(matches!(
        zero_index.validate(),
        Err(NodeError::SealedCommittee { .. })
    ));

    let mut duplicated = CommitteeRecord::from_committee(&committee);
    duplicated.members[1].0 = duplicated.members[0].0;
    assert!(matches!(
        duplicated.validate(),
        Err(NodeError::SealedCommittee { .. })
    ));

    let mut rubbish = CommitteeRecord::from_committee(&committee);
    rubbish.encryption_key = [0xFFu8; 32];
    assert!(matches!(
        rubbish.validate(),
        Err(NodeError::SealedCommittee { .. })
    ));
}

#[test]
fn an_envelope_payload_survives_the_wire() {
    let envelope = SealedEnvelope {
        reveal_height: 12_345,
        ciphertext: vec![0xABu8; 96],
    };
    let kind = TxKind::Seal(Box::new(envelope.clone()));

    let mut bytes = Vec::new();
    kind.encode_into(&mut bytes);
    let mut reader = ByteReader::new(&bytes);
    let decoded = TxKind::decode(&mut reader).expect("decode");
    reader.finish().expect("no trailing bytes");

    assert_eq!(decoded, kind);

    let share = RevealShare {
        envelope: [3u8; 32],
        member: 4_097,
        share: [5u8; 32],
        proof: [6u8; 64],
    };
    let kind = TxKind::RevealShare(share);
    let mut bytes = Vec::new();
    kind.encode_into(&mut bytes);
    let mut reader = ByteReader::new(&bytes);
    assert_eq!(TxKind::decode(&mut reader).expect("decode"), kind);
    reader.finish().expect("no trailing bytes");
}

#[test]
fn the_sealed_layer_changes_the_state_root_only_when_it_holds_something() {
    let sender = generate_signing_key().expect("key");
    let fixture = fixture(&[(sender.address(), 1_000_000)]);

    // A committee is itself an `m:` record, so the root already reflects the
    // layer. What this pins is that a *pending envelope* moves it, and that
    // opening or expiring one moves it back — the layer is state, not a
    // decoration on state.
    let before = fixture.db.state_root().expect("root");

    let (envelope, id) = fixture.seal(&TxKind::Transfer, &sender, 0, 4);
    fixture.apply(vec![envelope], 1).expect("include");
    let pending = fixture.db.state_root().expect("root");
    assert_ne!(before, pending);

    fixture.apply(vec![], 4).expect("expire");
    let after = fixture.db.state_root().expect("root");
    assert_ne!(pending, after);
    assert!(!fixture.is_pending(&id, 4));
}

#[test]
fn share_and_envelope_keys_order_by_height_then_identity() {
    // The execution order of every revealed transaction. It has to be a
    // property of the key bytes, because that is all the settle pass sees.
    let low = envelope_key(9, &[0u8; 32]);
    let high = envelope_key(10, &[0u8; 32]);
    assert!(low < high, "a big-endian height must sort numerically");

    let first = envelope_key(9, &[1u8; 32]);
    let second = envelope_key(9, &[2u8; 32]);
    assert!(first < second);

    // Every share for one envelope must sit under one contiguous prefix, or
    // gathering them would be a scan of the whole subsystem.
    let a = share_key(9, &[1u8; 32], 1);
    let b = share_key(9, &[1u8; 32], 2);
    let elsewhere = share_key(9, &[2u8; 32], 1);
    assert!(a < b && b < elsewhere);
}
