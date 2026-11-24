//! DAG-BFT vertex digest and certificate record — `spec/05-consensus.md`
//! CON-18 and CON-19 (ADR-155). Written from the spec text, not from the
//! node's `maya_dag_bft` or `consensus::bft::wire`.

/// A vertex as CON-18 hashes it.
pub struct Vertex<'a> {
    /// Committee epoch.
    pub epoch: u64,
    /// DAG round.
    pub round: u64,
    /// Proposer's committee index.
    pub author: u32,
    /// Proposer's clock, milliseconds.
    pub timestamp_ms: u64,
    /// Parent certificate digests, sorted.
    pub parents: &'a [[u8; 32]],
    /// Opaque transactions.
    pub batch: &'a [Vec<u8>],
}

fn derive(context: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(context);
    for p in parts {
        h.update(p);
    }
    *h.finalize().as_bytes()
}

/// CON-18: `parents_root = derive("maya2c dag-bft vertex parents v1", n_le64 ‖ parent*n)`.
#[must_use]
pub fn parents_root(parents: &[[u8; 32]]) -> [u8; 32] {
    let mut bytes = (parents.len() as u64).to_le_bytes().to_vec();
    for p in parents {
        bytes.extend_from_slice(p);
    }
    derive("maya2c dag-bft vertex parents v1", &[&bytes])
}

/// CON-18: `batch_root = derive("maya2c dag-bft vertex batch v1", m_le64 ‖ blake3(tx)*m)`.
#[must_use]
pub fn batch_root(batch: &[Vec<u8>]) -> [u8; 32] {
    let mut bytes = (batch.len() as u64).to_le_bytes().to_vec();
    for tx in batch {
        bytes.extend_from_slice(blake3::hash(tx).as_bytes());
    }
    derive("maya2c dag-bft vertex batch v1", &[&bytes])
}

/// CON-18: `blake3("maya2c/dag-bft/vertex/v3" ‖ epoch ‖ round ‖ author_le32 ‖
/// timestamp_ms ‖ parents_root ‖ batch_root)`.
#[must_use]
pub fn vertex_digest(v: &Vertex<'_>) -> [u8; 32] {
    let mut bytes = b"maya2c/dag-bft/vertex/v3".to_vec();
    bytes.extend_from_slice(&v.epoch.to_le_bytes());
    bytes.extend_from_slice(&v.round.to_le_bytes());
    bytes.extend_from_slice(&v.author.to_le_bytes());
    bytes.extend_from_slice(&v.timestamp_ms.to_le_bytes());
    bytes.extend_from_slice(&parents_root(v.parents));
    bytes.extend_from_slice(&batch_root(v.batch));
    *blake3::hash(&bytes).as_bytes()
}

/// CON-19: `0x01 ‖ vertex ‖ k_le64 ‖ (voter_le32 ‖ sig_len_le64 ‖ sig)*k`,
/// with `vertex = epoch ‖ round ‖ author_le32 ‖ timestamp_ms ‖ n_le64 ‖
/// parent*n ‖ m_le64 ‖ (len_le64 ‖ tx)*m`.
#[must_use]
pub fn certificate_record(v: &Vertex<'_>, votes: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = vec![0x01];
    out.extend_from_slice(&v.epoch.to_le_bytes());
    out.extend_from_slice(&v.round.to_le_bytes());
    out.extend_from_slice(&v.author.to_le_bytes());
    out.extend_from_slice(&v.timestamp_ms.to_le_bytes());
    out.extend_from_slice(&(v.parents.len() as u64).to_le_bytes());
    for p in v.parents {
        out.extend_from_slice(p);
    }
    out.extend_from_slice(&(v.batch.len() as u64).to_le_bytes());
    for tx in v.batch {
        out.extend_from_slice(&(tx.len() as u64).to_le_bytes());
        out.extend_from_slice(tx);
    }
    out.extend_from_slice(&(votes.len() as u64).to_le_bytes());
    for (voter, sig) in votes {
        out.extend_from_slice(&voter.to_le_bytes());
        out.extend_from_slice(&(sig.len() as u64).to_le_bytes());
        out.extend_from_slice(sig);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_a_byte_between_batch_items_changes_the_digest() {
        let base = |batch: &[Vec<u8>]| {
            vertex_digest(&Vertex {
                epoch: 1,
                round: 2,
                author: 3,
                timestamp_ms: 4,
                parents: &[],
                batch,
            })
        };
        assert_ne!(base(&[vec![1, 2], vec![3]]), base(&[vec![1], vec![2, 3]]));
    }
}
