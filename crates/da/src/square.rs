//! The extended square, its header, repair, and sampling.

use crate::merkle::{CellProof, merkle_root};
use crate::Hash;

/// What a light client holds: the square size and the row/column roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// Original square side.
    pub k: usize,
    /// Root of each of the `2k` extended rows.
    pub row_roots: Vec<Hash>,
    /// Root of each of the `2k` extended columns.
    pub col_roots: Vec<Hash>,
}

impl Header {
    /// The 32-byte data root a block header commits to.
    pub fn data_root(&self) -> Hash {
        let mut h = blake3::Hasher::new_derive_key("maya2c/da/data-root/v1");
        h.update(&(self.k as u64).to_le_bytes());
        for r in self.row_roots.iter().chain(&self.col_roots) {
            h.update(r);
        }
        *h.finalize().as_bytes()
    }
}

/// A fully extended square.
#[derive(Clone, Debug)]
pub struct Extended {
    /// Header.
    pub header: Header,
    /// `2k × 2k` cells, row-major.
    pub cells: Vec<Vec<u8>>,
    /// Chunk size, bytes.
    pub chunk: usize,
}

fn rs_extend(originals: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let k = originals.len();
    // k, k are supported counts for every power of two used here.
    let recovery = reed_solomon_simd::encode(k, k, originals).unwrap_or_default();
    originals.iter().cloned().chain(recovery).collect()
}

impl Extended {
    /// Lays `data` out as a `k × k` square of `chunk`-byte cells (zero
    /// padded) and extends it. `k` must be a power of two; `chunk` even.
    pub fn encode(data: &[u8], k: usize, chunk: usize) -> Self {
        let w = 2 * k;
        let mut cells = vec![Vec::new(); w * w];
        // Rows 0..k: original data, extended to 2k columns.
        for r in 0..k {
            let row: Vec<Vec<u8>> = (0..k)
                .map(|c| {
                    let start = (r * k + c) * chunk;
                    let mut cell = vec![0u8; chunk];
                    if start < data.len() {
                        let end = (start + chunk).min(data.len());
                        cell[..end - start].copy_from_slice(&data[start..end]);
                    }
                    cell
                })
                .collect();
            for (c, cell) in rs_extend(&row).into_iter().enumerate() {
                cells[r * w + c] = cell;
            }
        }
        // Every column extended from k to 2k rows.
        for c in 0..w {
            let col: Vec<Vec<u8>> = (0..k).map(|r| cells[r * w + c].clone()).collect();
            for (r, cell) in rs_extend(&col).into_iter().enumerate().skip(k) {
                cells[r * w + c] = cell;
            }
        }
        let row_roots = (0..w)
            .map(|r| merkle_root(&(0..w).map(|c| cells[r * w + c].as_slice()).collect::<Vec<_>>()))
            .collect();
        let col_roots = (0..w)
            .map(|c| merkle_root(&(0..w).map(|r| cells[r * w + c].as_slice()).collect::<Vec<_>>()))
            .collect();
        Self { header: Header { k, row_roots, col_roots }, cells, chunk }
    }

    /// Side of the extended square.
    pub fn width(&self) -> usize {
        2 * self.header.k
    }

    /// A cell with its proof.
    pub fn cell(&self, row: usize, col: usize) -> (Vec<u8>, CellProof) {
        let w = self.width();
        let row_cells: Vec<&[u8]> = (0..w).map(|c| self.cells[row * w + c].as_slice()).collect();
        (self.cells[row * w + col].clone(), CellProof::build(row, col, &row_cells))
    }
}

/// A partially known square, for repair.
#[derive(Clone, Debug)]
pub struct Square {
    /// Header the cells are checked against.
    pub header: Header,
    /// `2k × 2k` cells, `None` where missing.
    pub cells: Vec<Option<Vec<u8>>>,
}

impl Square {
    /// Iteratively rebuilds rows and columns with at least `k` cells, until
    /// nothing changes. Returns `Ok(true)` when complete, `Ok(false)` when
    /// stuck (data unavailable), and `Err` when a rebuilt row or column does
    /// not match its committed root — an incorrectly encoded square, the
    /// evidence a bad-encoding fraud proof would carry.
    pub fn repair(&mut self) -> Result<bool, (bool, usize)> {
        let k = self.header.k;
        let w = 2 * k;
        loop {
            let mut progress = false;
            for is_row in [true, false] {
                for line in 0..w {
                    let idx = |i: usize| if is_row { line * w + i } else { i * w + line };
                    let present: Vec<(usize, Vec<u8>)> =
                        (0..w).filter_map(|i| self.cells[idx(i)].clone().map(|c| (i, c))).collect();
                    if present.len() == w || present.len() < k {
                        continue;
                    }
                    let originals = present.iter().filter(|(i, _)| *i < k).map(|(i, c)| (*i, c.as_slice()));
                    let recovery = present.iter().filter(|(i, _)| *i >= k).map(|(i, c)| (*i - k, c.as_slice()));
                    let Ok(restored) = reed_solomon_simd::decode(k, k, originals, recovery) else {
                        continue;
                    };
                    let mut full: Vec<Vec<u8>> = (0..k)
                        .map(|i| {
                            self.cells[idx(i)].clone().or_else(|| restored.get(&i).cloned()).unwrap_or_default()
                        })
                        .collect();
                    full = rs_extend(&full);
                    let root = merkle_root(&full.iter().map(Vec::as_slice).collect::<Vec<_>>());
                    let committed = if is_row { self.header.row_roots[line] } else { self.header.col_roots[line] };
                    if root != committed {
                        return Err((is_row, line));
                    }
                    for (i, cell) in full.into_iter().enumerate() {
                        if self.cells[idx(i)].is_none() {
                            self.cells[idx(i)] = Some(cell);
                            progress = true;
                        }
                    }
                }
            }
            if self.cells.iter().all(Option::is_some) {
                return Ok(true);
            }
            if !progress {
                return Ok(false);
            }
        }
    }
}

/// What a sampling light client concluded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// Every sampled cell arrived with a valid proof.
    Available,
    /// At least one sample was missing or failed its proof.
    Unavailable,
}

/// Samples `positions` through `fetch`, checking each proof against the
/// header's row roots.
pub fn sample(
    header: &Header,
    positions: &[(usize, usize)],
    fetch: impl Fn(usize, usize) -> Option<(Vec<u8>, CellProof)>,
) -> Sampling {
    let w = 2 * header.k;
    for &(r, c) in positions {
        let Some((cell, proof)) = fetch(r, c) else {
            return Sampling::Unavailable;
        };
        if proof.row != r || proof.col != c || !proof.verify(&header.row_roots[r], w, &cell) {
            return Sampling::Unavailable;
        }
    }
    Sampling::Available
}
