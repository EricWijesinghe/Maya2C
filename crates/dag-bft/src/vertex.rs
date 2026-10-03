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

    /// n − f: certificates needed to advance, votes needed to certify.
    ///
    /// Any two quorums must share an honest validator, or one equivocating
    /// author gets two certificates for one round from disjoint voters. Two
    /// sets of q overlap in 2q − n members, so safety needs 2q − n ≥ f + 1;
    /// liveness needs q ≤ n − f, since f may never answer. n − f meets both
    /// for every n ≥ 3f + 1. It equals the textbook 2f + 1 only when
    /// n = 3f + 1: at n = 6 (f = 1) 2f + 1 is 3, two disjoint halves (ADR-039).
    pub const fn quorum(self) -> u16 {
        self.size - self.faults()
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

/// One opaque transaction as the engine carries it. The engine never parses
/// it: the node decodes it when a committed sub-DAG becomes a block, and the
/// simulator's ledger reads a little-endian `u64` out of it.
pub type Payload = Vec<u8>;

/// One validator's proposal for one round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vertex {
    /// Committee epoch. Part of the digest, so a signature made for one
    /// epoch's DAG can never be replayed into the next one's.
    pub epoch: u64,
    /// DAG round; genesis is round 0.
    pub round: u64,
    /// Proposer.
    pub author: ValidatorId,
    /// The author's clock when it proposed, in milliseconds. Certified with
    /// the vertex, so every node reads the same value: the node derives a
    /// block's timestamp from its anchor's, never from its own clock.
    pub timestamp_ms: u64,
    /// Digests of at least a quorum (n − f) of certificates from `round - 1`, sorted.
    pub parents: Vec<Digest>,
    /// Transactions carried inline. Narwhal separates payload into worker
    /// batches referenced by digest; inline payload is the v1 simplification
    /// ADR-027 records, bounded by `Params::max_batch_bytes`.
    pub batch: Vec<Payload>,
}

impl Vertex {
    /// The genesis vertex every node creates identically for `author`.
    pub fn genesis(author: ValidatorId) -> Self {
        Self::genesis_in(0, author)
    }

    /// The genesis vertex of `epoch`'s DAG for `author`.
    pub fn genesis_in(epoch: u64, author: ValidatorId) -> Self {
        Self {
            epoch,
            round: 0,
            author,
            timestamp_ms: 0,
            parents: Vec::new(),
            batch: Vec::new(),
        }
    }

    /// Content digest; what votes and parent links refer to.
    pub fn digest(&self) -> Digest {
        let mut h = blake3::Hasher::new();
        h.update(b"maya2c/dag-bft/vertex/v2");
        h.update(&self.epoch.to_le_bytes());
        h.update(&self.round.to_le_bytes());
        h.update(&self.author.to_le_bytes());
        h.update(&self.timestamp_ms.to_le_bytes());
        h.update(&(self.parents.len() as u64).to_le_bytes());
        for p in &self.parents {
            h.update(p);
        }
        h.update(&(self.batch.len() as u64).to_le_bytes());
        for tx in &self.batch {
            h.update(&(tx.len() as u64).to_le_bytes());
            h.update(tx);
        }
        *h.finalize().as_bytes()
    }

    /// Bytes of payload carried.
    pub fn payload_bytes(&self) -> usize {
        self.batch.iter().map(Vec::len).sum()
    }
}

/// A vertex with a quorum of votes: proof it was reliably broadcast, so no
/// author can have two certified vertices in one round.
///
/// Each vote is a signature by the voter over the vertex digest, made and
/// checked through [`crate::Authenticator`]. The node signs with ML-DSA-65
/// and carries the quorum's signatures side by side (ADR-021 measured why they
/// are not aggregated); the simulator's [`crate::Unauthenticated`] leaves them
/// empty, and that is the one thing a simulation takes on trust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// The certified vertex.
    pub vertex: Vertex,
    /// Distinct voters, sorted.
    pub votes: Vec<ValidatorId>,
    /// `signatures[i]` is `votes[i]`'s signature over the vertex digest.
    pub signatures: Vec<Vec<u8>>,
}

impl Certificate {
    /// Genesis certificates need no votes; every node makes the same ones.
    pub fn genesis(committee: Committee) -> Vec<Self> {
        Self::genesis_in(0, committee)
    }

    /// `epoch`'s genesis certificates.
    pub fn genesis_in(epoch: u64, committee: Committee) -> Vec<Self> {
        (0..committee.size())
            .map(|a| Self {
                vertex: Vertex::genesis_in(epoch, a),
                votes: (0..committee.size()).collect(),
                signatures: vec![Vec::new(); usize::from(committee.size())],
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
        let paired = self.signatures.len() == self.votes.len();
        sorted_distinct
            && members
            && enough
            && parents_ok
            && paired
            && self.vertex.author < committee.size()
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
        // Between the 3f + 1 sizes the quorum still grows with n.
        let c = Committee::new(6);
        assert_eq!((c.faults(), c.quorum(), c.validity()), (1, 5, 2));
        let c = Committee::new(2);
        assert_eq!((c.faults(), c.quorum(), c.validity()), (0, 2, 1));
    }

    #[test]
    fn any_two_quorums_share_an_honest_validator_at_every_size() {
        for n in 1..=u16::MAX.min(1_000) {
            let c = Committee::new(n);
            let (f, q) = (u32::from(c.faults()), u32::from(c.quorum()));
            let n = u32::from(n);
            assert!(n >= 3 * f + 1, "n = {n}");
            // Two quorums overlap in at least 2q - n members: more than f.
            assert!(
                2 * q >= n + f + 1,
                "n = {n}: quorums of {q} may share only faulty members"
            );
            // The honest validators alone can form one.
            assert!(q <= n - f, "n = {n}: a quorum of {q} needs a faulty vote");
        }
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
            epoch: 0,
            round: 1,
            author: 0,
            timestamp_ms: 0,
            parents,
            batch: vec![],
        };
        let cert = |votes: Vec<ValidatorId>| Certificate {
            vertex: v.clone(),
            signatures: vec![Vec::new(); votes.len()],
            votes,
        };
        assert!(cert(vec![0, 1, 2]).is_well_formed(c));
        assert!(!cert(vec![0, 1]).is_well_formed(c));
        assert!(!cert(vec![0, 1, 1]).is_well_formed(c));
        assert!(!cert(vec![0, 1, 9]).is_well_formed(c));
        let mut unpaired = cert(vec![0, 1, 2]);
        unpaired.signatures.pop();
        assert!(!unpaired.is_well_formed(c), "a vote without its signature");
    }

    #[test]
    fn the_digest_binds_epoch_timestamp_and_payload_boundaries() {
        let base = Vertex::genesis_in(3, 1);
        let mut other_epoch = base.clone();
        other_epoch.epoch = 4;
        let mut other_time = base.clone();
        other_time.timestamp_ms = 1;
        assert_ne!(base.digest(), other_epoch.digest());
        assert_ne!(base.digest(), other_time.digest());
        // Length-prefixed, so moving a byte between two payloads changes it.
        let mut split_a = base.clone();
        split_a.batch = vec![vec![1, 2], vec![3]];
        let mut split_b = base;
        split_b.batch = vec![vec![1], vec![2, 3]];
        assert_ne!(split_a.digest(), split_b.digest());
    }
}
