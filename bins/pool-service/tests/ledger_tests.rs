//! Ledger conformance, run against both implementations.
//!
//! Every test here runs twice: once against [`MemoryLedger`] and once against
//! [`RocksLedger`]. That is the point of the file. A trait with two
//! implementations and one tested implementation is a trait with one
//! implementation and a liability, and the liability here is the durable one —
//! the tests would be passing against the store that does not hold the money.
//!
//! The crash-safety tests are the exception and run only against RocksDB, since
//! reopening a `MemoryLedger` is not a meaningful operation.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::consensus::U256;
use custom_l1_node::state::Address;
use maya_pool_service::ledger::memory::MemoryLedger;
use maya_pool_service::ledger::rocks::RocksLedger;
use maya_pool_service::ledger::{BlockState, FoundBlock, ShareLedger};
use maya_pool_service::model::{PayoutBatch, PayoutEntry, PayoutState};
use tempfile::TempDir;

const ALICE: Address = [0xAA; 32];
const BOB: Address = [0xBB; 32];

/// Runs `body` against both implementations.
fn both(body: impl Fn(&dyn ShareLedger)) {
    body(&MemoryLedger::new());

    let dir = TempDir::new().expect("temp dir");
    let rocks = RocksLedger::open(dir.path()).expect("open rocks ledger");
    body(&rocks);
}

fn found_block(id: &str, height: u64, sequence: u64, reward: u64) -> FoundBlock {
    FoundBlock {
        id: id.to_string(),
        height,
        sequence,
        reward,
        finder: ALICE,
        found_at_millis: 1_700_000_000_000,
        state: BlockState::Immature,
    }
}

fn entry(miner: Address, amount: u64) -> PayoutEntry {
    PayoutEntry { miner, amount }
}

fn batch(id: u64, nonce: u64, state: PayoutState, entries: Vec<PayoutEntry>) -> PayoutBatch {
    PayoutBatch {
        id,
        nonce,
        entries,
        signed_tx: vec![1, 2, 3],
        txid: Some(format!("tx{id}")),
        state,
        created_at_millis: 1_700_000_000_000,
        included_height: None,
    }
}

#[test]
fn share_sequences_are_dense_and_monotonic() {
    both(|ledger| {
        for expected in 0..10u64 {
            let sequence = ledger.append_share(&ALICE, "rig", 1_024, expected).unwrap();
            assert_eq!(sequence, expected);
        }
        assert_eq!(ledger.tip_sequence().unwrap(), Some(9));
    });
}

#[test]
fn an_empty_ledger_has_no_tip() {
    both(|ledger| assert_eq!(ledger.tip_sequence().unwrap(), None));
}

#[test]
fn a_window_reads_backwards_from_the_share_that_found_the_block() {
    // Not from the ledger tip. Shares submitted after a block was found belong
    // to the next block's window, and paying them from this one pays twice.
    both(|ledger| {
        for index in 0..20u64 {
            ledger.append_share(&ALICE, "rig", 10, index).unwrap();
        }

        let slice = ledger.window(9, U256::from_u64(50)).unwrap();
        assert_eq!(slice.shares.len(), 5);
        for share in &slice.shares {
            assert!(
                share.sequence <= 9,
                "window reached past the found share: {}",
                share.sequence
            );
        }
    });
}

#[test]
fn a_window_walks_in_numeric_order_past_a_byte_order_boundary() {
    // The regression this guards: little-endian keys would sort share 256
    // before share 2, and the window would silently contain the wrong shares.
    both(|ledger| {
        for index in 0..300u64 {
            ledger.append_share(&ALICE, "rig", 1, index).unwrap();
        }

        let slice = ledger.window(299, U256::from_u64(5)).unwrap();
        let sequences: Vec<u64> = slice.shares.iter().map(|share| share.sequence).collect();
        assert_eq!(sequences, vec![299, 298, 297, 296, 295]);
    });
}

#[test]
fn credits_start_immature_and_mature_together() {
    both(|ledger| {
        let block = found_block("block-a", 100, 0, 1_000);
        ledger
            .record_block(&block, &[entry(ALICE, 600), entry(BOB, 400)])
            .unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 600);
        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 0);

        ledger.mature_block("block-a").unwrap();

        let alice = ledger.balance(&ALICE).unwrap();
        assert_eq!(alice.immature, 0);
        assert_eq!(alice.unpaid, 600);
        assert_eq!(ledger.balance(&BOB).unwrap().unpaid, 400);
    });
}

#[test]
fn maturing_twice_does_not_credit_twice() {
    // The confirmation watcher legitimately sees a block cross the threshold
    // more than once: after a restart, or when two polls overlap.
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();

        ledger.mature_block("block-a").unwrap();
        ledger.mature_block("block-a").unwrap();
        ledger.mature_block("block-a").unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 600);
    });
}

#[test]
fn an_orphaned_block_takes_its_credits_back() {
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();

        ledger.orphan_block("block-a").unwrap();

        let alice = ledger.balance(&ALICE).unwrap();
        assert_eq!(alice.immature, 0);
        assert_eq!(alice.unpaid, 0, "an orphaned block earns nothing");
        assert_eq!(
            ledger.block("block-a").unwrap().unwrap().state,
            BlockState::Orphaned
        );
    });
}

#[test]
fn orphaning_twice_does_not_take_credits_from_another_block() {
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger
            .record_block(&found_block("block-b", 101, 1, 1_000), &[entry(ALICE, 500)])
            .unwrap();

        ledger.orphan_block("block-a").unwrap();
        ledger.orphan_block("block-a").unwrap();

        // Block B's credit must be untouched.
        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 500);
    });
}

#[test]
fn a_matured_block_cannot_be_orphaned_out_of_paid_credits() {
    // Once credits are payable they may already be inside a batch. Reversing
    // them at that point would take money the pool has committed to send.
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger.mature_block("block-a").unwrap();
        ledger.orphan_block("block-a").unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 600);
        assert_eq!(
            ledger.block("block-a").unwrap().unwrap().state,
            BlockState::Mature
        );
    });
}

#[test]
fn recording_the_same_block_twice_credits_once() {
    both(|ledger| {
        let block = found_block("block-a", 100, 0, 1_000);
        ledger.record_block(&block, &[entry(ALICE, 600)]).unwrap();
        ledger.record_block(&block, &[entry(ALICE, 600)]).unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().immature, 600);
    });
}

#[test]
fn creating_a_batch_debits_it_and_reversing_it_gives_the_credits_back() {
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger.mature_block("block-a").unwrap();

        let mut payout = batch(0, 0, PayoutState::Signed, vec![entry(ALICE, 600)]);
        ledger.create_batch(&payout).unwrap();

        let alice = ledger.balance(&ALICE).unwrap();
        assert_eq!(alice.unpaid, 0, "the credit is committed to a batch");
        assert_eq!(alice.paid, 600);

        payout.state = PayoutState::Orphaned;
        ledger.reverse_batch(&payout).unwrap();

        let alice = ledger.balance(&ALICE).unwrap();
        assert_eq!(alice.unpaid, 600, "a lost batch owes the miner again");
        assert_eq!(alice.paid, 0);
    });
}

#[test]
fn a_repeated_batch_creation_debits_once() {
    // The reason creation and the debit are one write: a resumed daemon
    // re-runs the creation of a batch it already wrote down, and debiting a
    // second time would take credits the miner is still owed.
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger.mature_block("block-a").unwrap();

        let payout = batch(0, 0, PayoutState::Signed, vec![entry(ALICE, 400)]);
        ledger.create_batch(&payout).unwrap();
        ledger.create_batch(&payout).unwrap();

        let alice = ledger.balance(&ALICE).unwrap();
        assert_eq!(alice.unpaid, 200);
        assert_eq!(alice.paid, 400);
    });
}

#[test]
fn reversing_a_batch_twice_credits_once() {
    // A reorg seen twice — two polls, or a poll either side of a restart —
    // must not pay the miner twice for one lost batch.
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger.mature_block("block-a").unwrap();

        let mut payout = batch(0, 0, PayoutState::Submitted, vec![entry(ALICE, 600)]);
        ledger.create_batch(&payout).unwrap();

        payout.state = PayoutState::Orphaned;
        ledger.reverse_batch(&payout).unwrap();
        ledger.reverse_batch(&payout).unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 600);
    });
}

#[test]
fn a_confirmed_batch_cannot_be_reversed() {
    // Confirmed means the transaction is buried. Crediting the miner again
    // would pay them twice for one batch.
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger.mature_block("block-a").unwrap();

        let mut payout = batch(0, 0, PayoutState::Submitted, vec![entry(ALICE, 600)]);
        ledger.create_batch(&payout).unwrap();

        payout.state = PayoutState::Confirmed;
        ledger.put_batch(&payout).unwrap();

        payout.state = PayoutState::Orphaned;
        ledger.reverse_batch(&payout).unwrap();

        assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 0);
        assert_eq!(ledger.balance(&ALICE).unwrap().paid, 600);
    });
}

#[test]
fn batch_ids_are_never_reissued() {
    // A reissued id would let one batch overwrite another's record, and the
    // overwritten one is a signed transaction nobody is watching any more.
    both(|ledger| {
        let ids: Vec<u64> = (0..5).map(|_| ledger.next_batch_id().unwrap()).collect();
        assert_eq!(ids, vec![0, 1, 2, 3, 4]);
    });
}

#[test]
fn open_batches_are_the_crash_recovery_list() {
    both(|ledger| {
        ledger
            .put_batch(&batch(0, 0, PayoutState::Confirmed, vec![entry(ALICE, 1)]))
            .unwrap();
        ledger
            .put_batch(&batch(1, 1, PayoutState::Signed, vec![entry(ALICE, 2)]))
            .unwrap();
        ledger
            .put_batch(&batch(2, 2, PayoutState::Submitted, vec![entry(BOB, 3)]))
            .unwrap();
        ledger
            .put_batch(&batch(3, 3, PayoutState::Failed, vec![entry(BOB, 4)]))
            .unwrap();

        let open: Vec<u64> = ledger
            .open_batches()
            .unwrap()
            .iter()
            .map(|batch| batch.id)
            .collect();

        // Signed but not broadcast, and broadcast but not confirmed. Both need
        // the daemon to do something; the other two do not.
        assert_eq!(open, vec![1, 2]);
    });
}

#[test]
fn balances_lists_only_miners_with_something_to_show() {
    both(|ledger| {
        ledger
            .record_block(&found_block("block-a", 100, 0, 1_000), &[entry(ALICE, 600)])
            .unwrap();

        let balances = ledger.balances().unwrap();
        assert_eq!(balances.len(), 1);
        assert_eq!(balances[0].0, ALICE);
    });
}

#[test]
fn pruning_drops_old_shares_and_leaves_the_window_intact() {
    both(|ledger| {
        for index in 0..100u64 {
            ledger.append_share(&ALICE, "rig", 10, index).unwrap();
        }

        let before = ledger.window(99, U256::from_u64(300)).unwrap();
        let removed = ledger.prune_shares_below(50).unwrap();
        let after = ledger.window(99, U256::from_u64(300)).unwrap();

        assert_eq!(removed, 50);
        // Pruning is wholesale removal from the old end, so a retained window
        // says exactly what it said before.
        assert_eq!(before.shares, after.shares);
    });
}

#[test]
fn concurrent_appends_never_share_a_sequence() {
    // The regression this guards is a silent credit transfer. RocksDB gives
    // atomic writes, not atomic read-modify-write; without a guard around the
    // sequence counter, two validator threads accepting shares at the same
    // moment read the same sequence and the second write overwrites the first
    // miner's record with the second miner's. Nothing errors, and one miner is
    // simply paid for the other's work.
    let dir = TempDir::new().expect("temp dir");
    let ledger = std::sync::Arc::new(RocksLedger::open(dir.path()).unwrap());

    let threads: Vec<_> = (0..8)
        .map(|thread| {
            let ledger = std::sync::Arc::clone(&ledger);
            std::thread::spawn(move || {
                for _ in 0..200 {
                    let miner = [thread as u8; 32];
                    ledger.append_share(&miner, "rig", 1, 0).unwrap();
                }
            })
        })
        .collect();

    for thread in threads {
        thread.join().expect("a writer panicked");
    }

    let total = 8 * 200;
    assert_eq!(ledger.tip_sequence().unwrap(), Some(total - 1));

    let window = ledger.window(total - 1, U256::MAX).unwrap();
    assert_eq!(
        window.shares.len() as u64,
        total,
        "shares were overwritten by a shared sequence"
    );
}

#[test]
fn a_reopened_ledger_still_knows_what_it_owes() {
    // The one property `MemoryLedger` cannot demonstrate, and the reason the
    // durable implementation exists.
    let dir = TempDir::new().expect("temp dir");

    {
        let ledger = RocksLedger::open(dir.path()).unwrap();
        for index in 0..10u64 {
            ledger.append_share(&ALICE, "rig", 100, index).unwrap();
        }
        ledger
            .record_block(&found_block("block-a", 100, 9, 1_000), &[entry(ALICE, 600)])
            .unwrap();
        ledger.mature_block("block-a").unwrap();
    }

    let ledger = RocksLedger::open(dir.path()).unwrap();
    assert_eq!(ledger.balance(&ALICE).unwrap().unpaid, 600);
    assert_eq!(ledger.tip_sequence().unwrap(), Some(9));
    assert_eq!(
        ledger.block("block-a").unwrap().unwrap().state,
        BlockState::Mature
    );
}

#[test]
fn a_reopened_ledger_does_not_reissue_a_share_sequence() {
    // A reused sequence would overwrite a share record: one miner's credit
    // replaced by another's, with nothing to show it happened.
    let dir = TempDir::new().expect("temp dir");

    {
        let ledger = RocksLedger::open(dir.path()).unwrap();
        for index in 0..5u64 {
            ledger.append_share(&ALICE, "rig", 100, index).unwrap();
        }
    }

    let ledger = RocksLedger::open(dir.path()).unwrap();
    assert_eq!(ledger.append_share(&BOB, "rig", 100, 5).unwrap(), 5);
}

#[test]
fn a_reopened_ledger_does_not_reissue_a_batch_id() {
    let dir = TempDir::new().expect("temp dir");

    {
        let ledger = RocksLedger::open(dir.path()).unwrap();
        assert_eq!(ledger.next_batch_id().unwrap(), 0);
        assert_eq!(ledger.next_batch_id().unwrap(), 1);
    }

    let ledger = RocksLedger::open(dir.path()).unwrap();
    assert_eq!(ledger.next_batch_id().unwrap(), 2);
}

#[test]
fn a_signed_batch_survives_a_restart_byte_for_byte() {
    // What makes a crash between signing and broadcast safe: the resumed daemon
    // rebroadcasts these exact bytes rather than signing a second transaction
    // against the same nonce.
    let dir = TempDir::new().expect("temp dir");
    let signed = vec![9u8; 512];

    {
        let ledger = RocksLedger::open(dir.path()).unwrap();
        let mut pending = batch(0, 42, PayoutState::Signed, vec![entry(ALICE, 600)]);
        pending.signed_tx = signed.clone();
        ledger.put_batch(&pending).unwrap();
    }

    let ledger = RocksLedger::open(dir.path()).unwrap();
    let recovered = ledger.open_batches().unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].signed_tx, signed);
    assert_eq!(recovered[0].nonce, 42);
}
