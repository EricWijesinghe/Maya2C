//! Availability sampling against withholding proposers
//! (Master Prompt 13 DONE WHEN: "DA withholding test passes in sim/").

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_possible_wrap)]

use std::collections::BTreeSet;

use maya_da::{Extended, Sampling, Square, sample};
use maya_sim::SimRng;

const K: usize = 16;
const CHUNK: usize = 256;
const SAMPLES: usize = 16;
const CLIENTS: usize = 2_000;

fn block() -> Vec<u8> {
    (0..K * K * CHUNK).map(|i| (i * 31 % 251) as u8).collect()
}

fn clients_detecting(ext: &Extended, withheld: &BTreeSet<(usize, usize)>, rng: &mut SimRng) -> usize {
    let w = ext.width() as u64;
    (0..CLIENTS)
        .filter(|_| {
            let positions: Vec<(usize, usize)> =
                (0..SAMPLES).map(|_| (rng.below(w) as usize, rng.below(w) as usize)).collect();
            let fetch = |r, c| (!withheld.contains(&(r, c))).then(|| ext.cell(r, c));
            sample(&ext.header, &positions, fetch) == Sampling::Unavailable
        })
        .count()
}

#[test]
fn every_cell_proves_and_a_tampered_cell_does_not() {
    let ext = Extended::encode(&block(), K, CHUNK);
    let w = ext.width();
    for r in (0..w).step_by(5) {
        for c in (0..w).step_by(3) {
            let (cell, proof) = ext.cell(r, c);
            assert!(proof.verify(&ext.header.row_roots[r], w, &cell));
            let mut bad = cell.clone();
            bad[0] ^= 1;
            assert!(!proof.verify(&ext.header.row_roots[r], w, &bad));
        }
    }
}

#[test]
fn a_quarter_of_cells_lost_at_random_is_repaired() {
    let ext = Extended::encode(&block(), K, CHUNK);
    let mut rng = SimRng::new(11);
    let cells = ext.cells.iter().map(|c| (rng.below(100) >= 25).then(|| c.clone())).collect();
    let mut sq = Square { header: ext.header.clone(), cells };
    assert_eq!(sq.repair(), Ok(true));
    assert!(sq.cells.iter().zip(&ext.cells).all(|(a, b)| a.as_ref() == Some(b)));
}

#[test]
fn a_proposer_withholding_30_percent_is_caught_by_sampling_clients() {
    let ext = Extended::encode(&block(), K, CHUNK);
    let w = ext.width();
    let mut rng = SimRng::new(0xDA_30);
    // Withhold a random 30% of cells.
    let mut withheld = BTreeSet::new();
    while withheld.len() < w * w * 30 / 100 {
        withheld.insert((rng.below(w as u64) as usize, rng.below(w as u64) as usize));
    }
    let detected = clients_detecting(&ext, &withheld, &mut rng);
    let rate = detected as f64 / CLIENTS as f64;
    let expected = 1.0 - 0.7f64.powi(SAMPLES as i32);
    println!("30% withheld: {detected}/{CLIENTS} clients detected ({rate:.4}); theory {expected:.4} for {SAMPLES} samples");
    assert!(rate > 0.99, "detection {rate}");
    // And an honest proposer is never flagged.
    assert_eq!(clients_detecting(&ext, &BTreeSet::new(), &mut rng), 0);
}

#[test]
fn the_minimal_unrecoverable_withholding_is_unrecoverable_and_still_detected() {
    let ext = Extended::encode(&block(), K, CHUNK);
    let w = ext.width();
    // A (k+1)x(k+1) sub-square: every row and column it touches keeps only
    // k-1 cells there, so neither can be rebuilt.
    let withheld: BTreeSet<(usize, usize)> = (0..=K).flat_map(|r| (0..=K).map(move |c| (r, c))).collect();
    let cells = (0..w * w)
        .map(|i| (!withheld.contains(&(i / w, i % w))).then(|| ext.cells[i].clone()))
        .collect();
    let mut sq = Square { header: ext.header.clone(), cells };
    assert_eq!(sq.repair(), Ok(false), "the data really is unavailable");
    let mut rng = SimRng::new(5);
    let detected = clients_detecting(&ext, &withheld, &mut rng);
    let share = withheld.len() as f64 / (w * w) as f64;
    let expected = 1.0 - (1.0 - share).powi(SAMPLES as i32);
    let rate = detected as f64 / CLIENTS as f64;
    println!("minimal withholding ({:.1}% of cells): detection {rate:.4}, theory {expected:.4}", share * 100.0);
    assert!(rate > expected - 0.03);
}

#[test]
fn an_incorrectly_extended_square_is_caught_on_repair() {
    let mut ext = Extended::encode(&block(), K, CHUNK);
    let w = ext.width();
    // The proposer commits roots over a square whose parity cell is wrong...
    ext.cells[3 * w + (w - 1)][0] ^= 0xFF;
    let row_cells: Vec<&[u8]> = (0..w).map(|c| ext.cells[3 * w + c].as_slice()).collect();
    ext.header.row_roots[3] = maya_da::merkle_root(&row_cells);
    // ...and a full node rebuilding that row from its first k cells finds the
    // committed root does not match the correct extension.
    let cells = (0..w * w)
        .map(|i| {
            let (r, c) = (i / w, i % w);
            (!(r == 3 && c >= K)).then(|| ext.cells[i].clone())
        })
        .collect();
    let mut sq = Square { header: ext.header.clone(), cells };
    assert!(matches!(sq.repair(), Err((true, 3) | (false, _))));
}
