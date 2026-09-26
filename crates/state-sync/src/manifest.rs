//! The manifest tree: chunk hashes under one root, and per-chunk proofs.

/// A chunk's hash.
pub fn chunk_hash(chunk: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c/state-sync/chunk/v1");
    h.update(chunk);
    *h.finalize().as_bytes()
}

fn node(l: &[u8; 32], r: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c/state-sync/node/v1");
    h.update(l);
    h.update(r);
    *h.finalize().as_bytes()
}

/// All chunk hashes and the tree above them. What a serving peer holds.
#[derive(Clone, Debug)]
pub struct Manifest {
    levels: Vec<Vec<[u8; 32]>>,
}

/// A chunk's path to the root. An odd node at a level's end is promoted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkProof {
    /// Chunk index.
    pub index: u32,
    /// Siblings bottom-up (levels where the node was promoted are skipped).
    pub siblings: Vec<[u8; 32]>,
}

impl Manifest {
    /// Builds the tree over chunk hashes, in chunk order.
    pub fn new(hashes: Vec<[u8; 32]>) -> Self {
        let mut levels = vec![hashes];
        while levels.last().is_some_and(|l| l.len() > 1) {
            let below = &levels[levels.len() - 1];
            let next = below
                .chunks(2)
                .map(|p| if let [l, r] = p { node(l, r) } else { p[0] })
                .collect();
            levels.push(next);
        }
        Self { levels }
    }

    /// The committed root.
    pub fn root(&self) -> [u8; 32] {
        self.levels
            .last()
            .and_then(|l| l.first().copied())
            .unwrap_or([0; 32])
    }

    /// Chunks in the snapshot.
    pub fn chunks(&self) -> u32 {
        u32::try_from(self.levels.first().map_or(0, Vec::len)).unwrap_or(u32::MAX)
    }

    /// Proof for chunk `index`.
    pub fn prove(&self, index: u32) -> ChunkProof {
        let mut i = index as usize;
        let mut siblings = Vec::new();
        for level in &self.levels[..self.levels.len().saturating_sub(1)] {
            if i ^ 1 < level.len() {
                siblings.push(level[i ^ 1]);
            }
            i /= 2;
        }
        ChunkProof { index, siblings }
    }
}

impl ChunkProof {
    /// Whether `chunk` is chunk `index` of `chunks` under `root`.
    pub fn verify(&self, root: &[u8; 32], chunks: u32, chunk: &[u8]) -> bool {
        if self.index >= chunks {
            return false;
        }
        let mut h = chunk_hash(chunk);
        let (mut i, mut width) = (self.index as usize, chunks as usize);
        let mut s = self.siblings.iter();
        while width > 1 {
            if i ^ 1 < width {
                let Some(sib) = s.next() else { return false };
                h = if i.is_multiple_of(2) {
                    node(&h, sib)
                } else {
                    node(sib, &h)
                };
            }
            i /= 2;
            width = width.div_ceil(2);
        }
        s.next().is_none() && h == *root
    }
}
