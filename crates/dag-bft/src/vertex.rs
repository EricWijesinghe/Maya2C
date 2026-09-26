//! Vertices, certificates and the committee that certifies them.

/// A 32-byte BLAKE3 digest.
pub type Digest = [u8; 32];

/// A validator's index in the committee.
pub type ValidatorId = u16;

/// A fixed validator set for one epoch. Equal stake: stake weighting is the
/// staking module's job, and the commit rule only needs the thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Committee {
    size: u16,
}

impl Committee {
    /// A committee of `size` validators. BFT needs at least four (f ≥ 1);
    /// fewer is allowed for tests and tolerates no fault.
    pub const fn new(size: u16) -> Self {
        Self { size }
    }

    /// Number of validators.
    pub const fn size(self) -> u16 {
        self.size
    }

    /// Faults tolerated: the largest f with n ≥ 3f + 1.
    pub const fn faults(self) -> u16 {
        self.size.saturating_sub(1) / 3
    }

    /// 2f + 1: certificates needed to advance, votes needed to certify.
    pub const fn quorum(self) -> u16 {
        2 * self.faults() + 1
    }

    /// f + 1: votes that commit an anchor (at least one honest).
    pub const fn validity(self) -> u16 {
        self.faults() + 1
    }

    /// Anchor author for an even `round`: round-robin, so every node computes
    /// the same leader with no communication. Odd rounds have no anchor.
    pub const fn leader(self, round: u64) -> Option<ValidatorId> {
        if !round.is_multiple_of(2) || self.size == 0 {
            return None;
        }
        // `size` fits u16, so the remainder does too.
        #[allow(clippy::cast_possible_truncation)]
        let leader = ((round / 2) % self.size as u64) as u16;
        Some(leader)
    }
}

/// One validator's proposal for one round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vertex {
    /// DAG round; genesis is round 0.
    pub round: u64,
    /// Proposer.
    pub author: ValidatorId,
    /// Digests of at least 2f + 1 certificates from `round - 1`, sorted.
    pub parents: Vec<Digest>,
    /// Transaction ids carried. In a node these are worker-batch digests
    /// (Narwhal): the vertex references payload, it never carries it.
    pub batch: Vec<u64>,
}

impl Vertex {
    /// The genesis vertex every node creates identically for `author`.
    pub fn genesis(author: ValidatorId) -> Self {
        Self {
            round: 0,
            author,
            parents: Vec::new(),
            batch: Vec::new(),
        }
    }

    /// Content digest; what votes and parent links refer to.
    pub fn digest(&self) -> Digest {
        let mut h = blake3::Hasher::new();
        h.update(b"maya2c/dag-bft/vertex/v1");
        h.update(&self.round.to_le_bytes());
        h.update(&self.author.to_le_bytes());
        h.update(&(self.parents.len() as u64).to_le_bytes());
        for p in &self.parents {
            h.update(p);
        }
        h.update(&(self.batch.len() as u64).to_le_bytes());
        for tx in &self.batch {
            h.update(&tx.to_le_bytes());
        }
        *h.finalize().as_bytes()
    }
}

/// A vertex with a quorum of votes: proof it was reliably broadcast, so no
/// author can have two certified vertices in one round.
///
/// Votes are validator ids here. In a node each vote is an ML-DSA signature
/// over the digest (Master Prompt 13 decides how the 2f + 1 of them are
/// aggregated); the simulator checks only the count, and that is the one
/// thing it takes on trust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// The certified vertex.
    pub vertex: Vertex,
    /// Distinct voters, sorted.
    pub votes: Vec<ValidatorId>,
}

impl Certificate {
    /// Genesis certificates need no votes; every node makes the same ones.
    pub fn genesis(committee: Committee) -> Vec<Self> {
        (0..committee.size())
            .map(|a| Self {
                vertex: Vertex::genesis(a),
                votes: (0..committee.size()).collect(),
            })
            .collect()
    }

    /// Digest of the certified vertex.
    pub fn digest(&self) -> Digest {
        self.vertex.digest()
    }

    /// Whether the votes are a quorum of distinct committee members and the
    /// vertex has the shape a round requires.
    pub fn is_well_formed(&self, committee: Committee) -> bool {
        let sorted_distinct = self.votes.windows(2).all(|w| w[0] < w[1]);
        let members = self.votes.iter().all(|v| *v < committee.size());
        let enough = self.votes.len() >= usize::from(committee.quorum());
        let parents_ok = if self.vertex.round == 0 {
            self.vertex.parents.is_empty()
        } else {
            self.vertex.parents.len() >= usize::from(committee.quorum())
                && self.vertex.parents.windows(2).all(|w| w[0] < w[1])
        };
        sorted_distinct && members && enough && parents_ok && self.vertex.author < committee.size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_match_the_bft_bounds() {
        let c = Committee::new(4);
        assert_eq!((c.faults(), c.quorum(), c.validity()), (1, 3, 2));
        let c = Committee::new(100);
        assert_eq!((c.faults(), c.quorum(), c.validity()), (33, 67, 34));
    }

    #[test]
    fn leaders_rotate_on_even_rounds_only() {
        let c = Committee::new(5);
        assert_eq!(c.leader(1), None);
        let leaders: Vec<_> = (0..6).map(|i| c.leader(2 * i)).collect();
        assert_eq!(leaders, [0, 1, 2, 3, 4, 0].map(Some));
    }

    #[test]
    fn a_certificate_short_of_quorum_or_with_duplicates_is_malformed() {
        let c = Committee::new(4);
        let g = Certificate::genesis(c);
        let parents: Vec<Digest> = {
            let mut p: Vec<_> = g.iter().map(Certificate::digest).collect();
            p.sort_unstable();
            p
        };
        let v = Vertex {
            round: 1,
            author: 0,
            parents,
            batch: vec![],
        };
        let ok = Certificate {
            vertex: v.clone(),
            votes: vec![0, 1, 2],
        };
        assert!(ok.is_well_formed(c));
        assert!(
            !Certificate {
                vertex: v.clone(),
                votes: vec![0, 1]
            }
            .is_well_formed(c)
        );
        assert!(
            !Certificate {
                vertex: v.clone(),
                votes: vec![0, 1, 1]
            }
            .is_well_formed(c)
        );
        assert!(
            !Certificate {
                vertex: v,
                votes: vec![0, 1, 9]
            }
            .is_well_formed(c)
        );
    }
}
