//! Conflicting transactions: parallel == serial (state, receipts, digest),
//! undeclared keys rejected, stale speculation rolled back.

#![allow(clippy::unwrap_used)]

use maya_parallel_exec::exec::BASE_GAS;
use maya_parallel_exec::{
    BlockResult, Key, Receipt, State, Tx, optimistic, result_digest, sequential, waves,
};
use proptest::prelude::*;

fn transfer(id: u64, from: Key, to: Key, amount: u64) -> Tx {
    Tx {
        id,
        kind: maya_loadgen::Kind::Transfer,
        sender: from,
        reads: vec![from, to],
        writes: vec![from, to],
        amount,
    }
}

fn base(pairs: &[(Key, u64)]) -> State {
    pairs.iter().copied().collect()
}

#[test]
fn stale_speculation_is_rolled_back_and_matches_serial() {
    // tx1 spends what tx0 deposits. Against the pre-block state tx1 sees
    // B == 0 and speculatively fails; the committed prefix says it succeeds.
    let b = base(&[(1, 100)]);
    let txs = [transfer(0, 1, 2, 100), transfer(1, 2, 3, 60)];
    let serial = sequential::execute(&b, &txs, 0);
    assert!(serial.receipts[1].success, "serial: tx1 funded by tx0");
    for threads in [1, 2, 4] {
        let (opt, stats) = optimistic::execute(&b, &txs, threads, 0);
        assert_eq!(opt, serial, "threads {threads}");
        assert_eq!(stats.reexecuted, 1, "tx1 re-executed, tx0 not");
        assert_eq!(opt.writes.get(&3), Some(&60), "rolled-back result applied");
    }
}

#[test]
fn independent_transactions_are_not_reexecuted() {
    let b = base(&[(1, 10), (3, 10)]);
    let txs = [transfer(0, 1, 2, 5), transfer(1, 3, 4, 5)];
    let (opt, stats) = optimistic::execute(&b, &txs, 1, 0);
    assert_eq!(stats.reexecuted, 0);
    assert_eq!(opt, sequential::execute(&b, &txs, 0));
}

#[test]
fn undeclared_key_is_rejected_with_base_gas_and_no_writes() {
    // Declares reads of key 1 only, but a transfer also reads key 2.
    let mut liar = transfer(0, 1, 2, 10);
    liar.reads = vec![1];
    let honest = transfer(1, 3, 4, 10);
    let b = base(&[(1, 100), (3, 100)]);
    let txs = [liar, honest];
    for threads in [1, 4] {
        let (r, _) = waves::execute(&b, &txs, threads, 0);
        assert_eq!(
            r.receipts[0],
            Receipt {
                success: false,
                gas: BASE_GAS
            }
        );
        assert!(!r.writes.contains_key(&1) && !r.writes.contains_key(&2));
        assert_eq!(r.writes.get(&3), Some(&90), "honest tx unaffected");
        assert!(r.receipts[1].success);
        assert_eq!(r, waves::sequential_with_declarations(&b, &txs, 0));
    }
    // Without the declaration rule the same tx would have applied.
    assert!(sequential::execute(&b, &txs, 0).receipts[0].success);
}

#[test]
fn digest_equal_for_equal_results_and_sensitive_to_every_field() {
    let b = base(&[(1, 100)]);
    let txs = [transfer(0, 1, 2, 30), transfer(1, 2, 3, 10)];
    let r = sequential::execute(&b, &txs, 0);
    assert_eq!(result_digest(&r), result_digest(&r.clone()));

    let mut v = r.clone();
    *v.writes.get_mut(&3).unwrap() += 1;
    assert_ne!(result_digest(&v), result_digest(&r), "write value");

    let mut g = r.clone();
    g.receipts[0].gas += 1;
    assert_ne!(result_digest(&g), result_digest(&r), "gas");

    let mut s = r.clone();
    s.receipts[1].success = !s.receipts[1].success;
    assert_ne!(result_digest(&s), result_digest(&r), "success flag");

    // Receipt order matters: build two distinct receipts and swap them.
    let mut ordered = r.clone();
    ordered.receipts = vec![
        Receipt {
            success: true,
            gas: 1,
        },
        Receipt {
            success: false,
            gas: 2,
        },
    ];
    let mut sw = ordered.clone();
    sw.receipts.swap(0, 1);
    assert_ne!(result_digest(&sw), result_digest(&ordered), "receipt order");

    let mut k = r.clone();
    let v3 = k.writes.remove(&3).unwrap();
    k.writes.insert(4, v3);
    assert_ne!(result_digest(&k), result_digest(&r), "key");

    assert_ne!(
        result_digest(&BlockResult {
            writes: State::new(),
            receipts: vec![]
        }),
        result_digest(&r)
    );
}

prop_compose! {
    // Few keys so most blocks conflict heavily; amounts straddle balances so
    // some transfers fail and later ones depend on earlier success.
    fn arb_block()(
        balances in proptest::collection::vec(0u64..200, 6),
        ops in proptest::collection::vec((0u64..6, 0u64..6, 0u64..150), 1..40),
    ) -> (State, Vec<Tx>) {
        let b = balances.iter().enumerate().map(|(k, v)| (k as Key, *v)).collect();
        let txs = ops
            .iter()
            .enumerate()
            .map(|(i, (f, t, a))| transfer(i as u64, *f, *t, *a))
            .collect();
        (b, txs)
    }
}

proptest! {
    #[test]
    fn optimistic_equals_serial_state_receipts_and_digest(
        (b, txs) in arb_block(),
        threads in 1usize..9,
    ) {
        let serial = sequential::execute(&b, &txs, 0);
        let (opt, _) = optimistic::execute(&b, &txs, threads, 0);
        prop_assert_eq!(result_digest(&opt), result_digest(&serial));
        prop_assert_eq!(opt, serial);
    }

    #[test]
    fn waves_equal_declared_serial_state_receipts_and_digest(
        (b, txs) in arb_block(),
        threads in 1usize..9,
    ) {
        let serial = waves::sequential_with_declarations(&b, &txs, 0);
        let (wv, _) = waves::execute(&b, &txs, threads, 0);
        prop_assert_eq!(result_digest(&wv), result_digest(&serial));
        prop_assert_eq!(wv, serial);
    }

    #[test]
    fn wave_assignment_never_puts_conflicting_transactions_together(
        (_b, txs) in arb_block(),
    ) {
        let w = waves::assign(&txs);
        for i in 0..txs.len() {
            for j in 0..i {
                let conflict = txs[i].writes.iter().any(|k| txs[j].writes.contains(k) || txs[j].reads.contains(k))
                    || txs[i].reads.iter().any(|k| txs[j].writes.contains(k));
                if conflict {
                    prop_assert!(w[i] > w[j], "tx {} must follow tx {}", i, j);
                }
            }
        }
    }
}
