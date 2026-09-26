//! Row and column Merkle trees over cells (power-of-two widths).

use crate::Hash;

fn leaf(cell: &[u8]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[0x00]);
    h.update(cell);
    *h.finalize().as_bytes()
}

fn node(l: &Hash, r: &Hash) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[0x01]);
    h.update(l);
    h.update(r);
    *h.finalize().as_bytes()
}

fn levels(cells: &[&[u8]]) -> Vec<Vec<Hash>> {
    let mut levels = vec![cells.iter().map(|c| leaf(c)).collect::<Vec<_>>()];
    while levels.last().is_some_and(|l| l.len() > 1) {
        let below = &levels[levels.len() - 1];
        let next = below.chunks(2).map(|p| node(&p[0], &p[1])).collect();
        levels.push(next);
    }
    levels
}

/// Root over `cells` (count must be a power of two).
pub fn merkle_root(cells: &[&[u8]]) -> Hash {
    levels(cells).last().and_then(|l| l.first().copied()).unwrap_or([0; 32])
}

/// A cell and its path to a row root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellProof {
    /// Row of the cell.
    pub row: usize,
    /// Column of the cell.
    pub col: usize,
    /// Siblings bottom-up.
    pub path: Vec<Hash>,
}

impl CellProof {
    /// Builds a proof for `col` within a row.
    pub fn build(row: usize, col: usize, row_cells: &[&[u8]]) -> Self {
        let lv = levels(row_cells);
        let mut i = col;
        let path = lv[..lv.len() - 1]
            .iter()
            .map(|l| {
                let s = l[i ^ 1];
                i /= 2;
                s
            })
            .collect();
        Self { row, col, path }
    }

    /// Whether `cell` sits at this position under `row_root`.
    pub fn verify(&self, row_root: &Hash, width: usize, cell: &[u8]) -> bool {
        if self.col >= width || (1usize << self.path.len()) != width {
            return false;
        }
        let mut h = leaf(cell);
        let mut i = self.col;
        for s in &self.path {
            h = if i.is_multiple_of(2) { node(&h, s) } else { node(s, &h) };
            i /= 2;
        }
        h == *row_root
    }
}
