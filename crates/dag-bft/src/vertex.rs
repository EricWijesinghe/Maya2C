//! Vertices, certificates and the committee that certifies them.

use std::sync::Arc;

/// A 32-byte BLAKE3 digest.
pub type Digest = [u8; 32];

/// A validator's index in the committee.
pub type ValidatorId = u16;

/// A fixed validator set for one epoch, with each member's voting weight.
///
/// Every threshold is dual (ADR-040 amendment 2026-11-24): it counts
/// weight (ADR-040) *and* heads. A quorum is W − f of the weight and n − f
/// of the members at once; validity is f + 1 of each. With equal weights —
/// [`Committee::new`] — the two coincide and the thresholds are the classic
/// ones. With stake as weight, a seat bought at the minimum bond carries
/// next to no say, and stake spread over a few addresses carries no quorum
/// without two thirds of the seats. The seats counted are *voters*, members
/// of positive weight: a zero-weight member has no say by either measure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committee {
    weights: Arc<[u64]>,
    total: u128,
    /// Members of positive weight: the `n` of the count rule.
    voters: u16,
    /// Fewest members whose weight reaches a quorum.
    min_quorum_size: u16,
}

impl Committee {
    /// A committee of `size` validators of equal weight. BFT needs at least
    /// four (f ≥ 1); fewer is allowed for tests and tolerates no fault.
    #[must_use]
    pub fn new(size: u16) -> Self {
        Self::from_weights(vec![1; usize::from(size)].into())
    }

    /// A committee whose member `i` votes with `weights[i]`, or `None` for
    /// more members than a `u16` validator id can name, or for members whose
    /// weights are all zero (every set of signers would be a quorum). The
    /// node must hand every peer the same weights (ADR-040 part 2).
    ///
    /// A zero weight is a seat with no say (Eric 2026-11-24 third round 5):
    /// the member keeps its index and its leader slot, and counts for
    /// neither the weight nor the seats of any threshold. That is how a
    /// tombstone held by the committee floor sits.
    #[must_use]
    pub fn weighted(weights: Vec<u64>) -> Option<Self> {
        u16::try_from(weights.len()).ok()?;
        if !weights.is_empty() && weights.iter().all(|w| *w == 0) {
            return None;
        }
        Some(Self::from_weights(weights.into()))
    }

    fn from_weights(weights: Arc<[u64]>) -> Self {
        let total: u128 = weights.iter().map(|w| u128::from(*w)).sum();
        let quorum = total - total.saturating_sub(1) / 3;
        let seats = weights.iter().filter(|w| **w > 0).count();
        let count_quorum = seats - seats.saturating_sub(1) / 3;
        let mut heaviest: Vec<u64> = weights.to_vec();
        heaviest.sort_unstable_by(|a, b| b.cmp(a));
        let mut reached = 0u128;
        let needed = heaviest
            .iter()
            .take_while(|w| {
                let short = reached < quorum;
                reached += u128::from(**w);
                short
            })
            .count();
        Self {
            weights,
            total,
            // At most the member count, which `weighted` bounds by u16::MAX.
            voters: u16::try_from(seats).unwrap_or(u16::MAX),
            min_quorum_size: u16::try_from(needed.max(count_quorum)).unwrap_or(u16::MAX),
        }
    }

    /// Fewest members that can be a quorum: the larger of the seat quorum
    /// n − f and the fewest members whose weight reaches W − f, the heaviest
    /// first. A head-count floor any quorum's member list must clear,
    /// checkable before anyone knows who the members are. Equal weights:
    /// n − f.
    #[must_use]
    pub fn min_quorum_size(&self) -> u16 {
        self.min_quorum_size
    }

    /// Members with a positive weight: the `n` of the count rule. A
    /// zero-weight seat (a tombstone the committee floor holds, Eric
    /// 2026-11-24 third round 5) is not a voter.
    #[must_use]
    pub fn voters(&self) -> u16 {
        self.voters
    }

    /// Distinct voters among `members`; zero-weight seats, strangers and
    /// repeats are not counted.
    pub fn voters_in(&self, members: impl IntoIterator<Item = ValidatorId>) -> u16 {
        // At most `size()`, which `weighted` bounds by u16::MAX.
        u16::try_from(self.tally(members).1).unwrap_or(u16::MAX)
    }

    /// Number of validators.
    #[must_use]
    pub fn size(&self) -> u16 {
        // `weighted` refuses more than u16::MAX members.
        u16::try_from(self.weights.len()).unwrap_or(u16::MAX)
    }

    /// Member `id`'s weight; zero for a non-member.
    #[must_use]
    pub fn weight(&self, id: ValidatorId) -> u64 {
        self.weights.get(usize::from(id)).copied().unwrap_or(0)
    }

    /// Total weight of `members`, each counted once however often it appears.
    pub fn weight_of(&self, members: impl IntoIterator<Item = ValidatorId>) -> u128 {
        self.tally(members).0
    }

    /// Weight and number of distinct voters among `members`; strangers,
    /// repeats and zero-weight seats count for nothing in either.
    fn tally(&self, members: impl IntoIterator<Item = ValidatorId>) -> (u128, usize) {
        let mut seen = vec![false; self.weights.len()];
        members
            .into_iter()
            .filter(|m| {
                self.weight(*m) > 0
                    && seen
                        .get_mut(usize::from(*m))
                        .is_some_and(|s| !std::mem::replace(s, true))
            })
            .fold((0, 0), |(w, n), m| (w + u128::from(self.weight(m)), n + 1))
    }

    /// Weight the committee tolerates being faulty: the largest f with
    /// W ≥ 3f + 1.
    #[must_use]
    pub fn faults(&self) -> u128 {
        self.total.saturating_sub(1) / 3
    }

    /// W − f: weight needed to certify a vertex or advance a round.
    ///
    /// Any two quorums must share honest weight, or one equivocating author
    /// gets two certificates for one round from disjoint voters. Two sets of
    /// weight q overlap in at least 2q − W, so safety needs 2q − W ≥ f + 1;
    /// liveness needs q ≤ W − f, since f may never answer. W − f meets both
    /// for every W ≥ 3f + 1. The textbook 2f + 1 equals it only when
    /// W = 3f + 1: at six equal members it is 3, two disjoint halves (ADR-039).
    #[must_use]
    pub fn quorum(&self) -> u128 {
        self.total - self.faults()
    }

    /// f + 1: weight that commits an anchor (some of it honest).
    #[must_use]
    pub fn validity(&self) -> u128 {
        self.faults() + 1
    }

    /// Seats the committee tolerates being faulty: the largest f with
    /// n ≥ 3f + 1, counted in voters whatever their positive weight.
    #[must_use]
    pub fn count_faults(&self) -> u16 {
        self.voters.saturating_sub(1) / 3
    }

    /// n − f: members a quorum needs besides its weight.
    #[must_use]
    pub fn count_quorum(&self) -> u16 {
        self.voters - self.count_faults()
    }

    /// f + 1 by members: seats an anchor's support needs besides its weight.
    #[must_use]
    pub fn count_validity(&self) -> u16 {
        self.count_faults() + 1
    }

    /// Whether `members` carry a quorum: W − f of the weight *and* n − f of
    /// the seats. Two such sets share more than f weight and more than f
    /// seats, so they share an honest member if the faulty side holds at
    /// most f by either measure (ADR-040 amendment 2026-11-24).
    pub fn is_quorum(&self, members: impl IntoIterator<Item = ValidatorId>) -> bool {
        let (weight, seats) = self.tally(members);
        weight >= self.quorum() && seats >= usize::from(self.count_quorum())
    }

    /// Whether `members` carry validity, the support that commits an
    /// anchor: f + 1 of the weight *and* f + 1 of the seats, so some of it
    /// is honest under either bound.
    pub fn is_validity(&self, members: impl IntoIterator<Item = ValidatorId>) -> bool {
        let (weight, seats) = self.tally(members);
        weight >= self.validity() && seats >= usize::from(self.count_validity())
    }

    /// Anchor author for an even `round`: round-robin over members, so every
    /// node computes the same leader with no communication. Odd rounds have
    /// no anchor. Not stake-weighted: an absent member costs one anchor
    /// timeout per turn until the epoch boundary jails it (ADR-040).
    #[must_use]
    pub fn leader(&self, round: u64) -> Option<ValidatorId> {
        let size = u64::from(self.size());
        if !round.is_multiple_of(2) || size == 0 {
            return None;
        }
        u16::try_from((round / 2) % size).ok()
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
/// checked through [`crate::Authenticator`]. The node signs with ML-DSA-87 (ADR-051 option 3)
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
    pub fn genesis(committee: &Committee) -> Vec<Self> {
        Self::genesis_in(0, committee)
    }

    /// `epoch`'s genesis certificates.
    pub fn genesis_in(epoch: u64, committee: &Committee) -> Vec<Self> {
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
    ///
    /// Parents are digests here, so their *weight* is checked where the DAG
    /// can name their authors (`Validator::has_parent_quorum`). This checks
    /// the head-count floor any quorum must clear, sorted and distinct,
    /// before a single signature is verified. With equal weights the floor
    /// is the quorum itself, exactly the check before ADR-040.
    pub fn is_well_formed(&self, committee: &Committee) -> bool {
        let sorted_distinct = self.votes.windows(2).all(|w| w[0] < w[1]);
        let members = self.votes.iter().all(|v| *v < committee.size());
        let enough = committee.is_quorum(self.votes.iter().copied());
        let parents_ok = if self.vertex.round == 0 {
            self.vertex.parents.is_empty()
        } else {
            self.vertex.parents.len() >= usize::from(committee.min_quorum_size())
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
        for n in 1..=1_000u16 {
            let c = Committee::new(n);
            let (f, q) = (c.faults(), c.quorum());
            let n = u128::from(n);
            assert!(n > 3 * f, "n = {n}");
            // Two quorums overlap in at least 2q - n members: more than f.
            assert!(
                2 * q > n + f,
                "n = {n}: quorums of {q} may share only faulty members"
            );
        }
    }

    #[test]
    fn neither_stake_alone_nor_seats_alone_make_a_quorum() {
        // ADR-040 and its dual-quorum amendment: one operator with 10^9
        // staked, three seats at 10^3 each. The cheap seats are three of
        // four heads but no stake; the bonded operator is all the stake but
        // one head. A quorum needs both.
        let c = Committee::weighted(vec![1_000_000_000, 1_000, 1_000, 1_000]).expect("4");
        assert!(!c.is_quorum([0]), "stake without seats is not a quorum");
        assert!(!c.is_quorum([1, 2, 3]), "seats without stake are not");
        assert!(c.is_quorum([0, 1, 2]), "stake and seats together are");
    }

    #[test]
    fn stake_split_over_a_few_capped_addresses_cannot_finalize_alone() {
        // Eric 2026-11-24 (third round): one party spreads its stake over
        // four operator addresses, each held to the 20% cap. That is 80% of
        // the weight on 4 of 10 seats: far past W − f by weight, short of
        // n − f = 7 by seats.
        let c = Committee::weighted(vec![200, 200, 200, 200, 34, 33, 33, 33, 33, 34]).expect("10");
        assert!(
            c.weight_of(0..4) >= c.quorum(),
            "the party holds a weight quorum"
        );
        assert_eq!(c.count_quorum(), 7);
        assert!(!c.is_quorum(0..4), "but not a quorum");
        assert!(c.is_quorum(0..7), "with three honest seats it is one");
    }

    #[test]
    fn dual_quorums_overlap_in_more_than_the_faulty_weight_and_the_faulty_seats() {
        let mut seed: u64 = 0xD0A1;
        for n in 1..=64usize {
            let weights: Vec<u64> = (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    (seed >> 44) + 1
                })
                .collect();
            let c = Committee::weighted(weights).expect("64 members at most");
            let (size, fc, qc) = (
                u128::from(c.size()),
                u128::from(c.count_faults()),
                u128::from(c.count_quorum()),
            );
            assert!(size > 3 * fc, "n = {n}");
            assert!(
                2 * qc > size + fc,
                "n = {n}: two quorums may share only faulty seats"
            );
            assert!(
                qc <= size - fc,
                "n = {n}: the honest seats cannot reach a quorum"
            );
            assert_eq!(c.count_validity(), c.count_faults() + 1);
            assert!(
                u128::from(c.min_quorum_size()) >= qc,
                "n = {n}: the head-count floor is below the seat quorum"
            );
        }
    }

    #[test]
    fn validity_needs_more_than_the_faulty_weight_and_the_faulty_seats() {
        // Seven seats: f = 2 by count. Two heavy members are past f + 1 by
        // weight but only f by seats; one more seat makes it valid.
        let c = Committee::weighted(vec![1_000, 1_000, 1, 1, 1, 1, 1]).expect("7");
        assert_eq!(c.count_validity(), 3);
        assert!(c.weight_of([0, 1]) >= c.validity());
        assert!(
            !c.is_validity([0, 1]),
            "two seats are within the faulty count"
        );
        assert!(c.is_validity([0, 1, 2]));
        assert!(
            !c.is_validity([2, 3, 4, 5, 6]),
            "seats without weight are not valid"
        );
    }

    #[test]
    fn equal_weights_make_the_two_quorums_one() {
        for n in 1..=200u16 {
            let c = Committee::new(n);
            assert_eq!(u128::from(c.count_quorum()), c.quorum(), "n = {n}");
            assert_eq!(u128::from(c.count_validity()), c.validity(), "n = {n}");
        }
    }

    #[test]
    fn weighted_quorums_always_overlap_in_more_than_the_faulty_weight() {
        // Deterministic spread of stakes, including skewed and equal ones.
        let mut seed: u64 = 0x5EED;
        for n in 1..=64usize {
            let weights: Vec<u64> = (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    (seed >> 40) + 1
                })
                .collect();
            let c = Committee::weighted(weights).expect("64 members at most");
            let (w, f, q) = (c.weight_of(0..c.size()), c.faults(), c.quorum());
            assert!(w > 3 * f, "n = {n}");
            assert!(
                2 * q > w + f,
                "n = {n}: quorums may share only faulty weight"
            );
            assert!(c.validity() > f, "n = {n}");
        }
    }

    #[test]
    fn a_member_counts_once_and_a_stranger_not_at_all() {
        let c = Committee::weighted(vec![5, 5, 5, 5]).expect("4");
        assert_eq!(c.weight_of([0, 0, 0]), 5);
        assert_eq!(c.weight_of([9]), 0);
        assert_eq!(c.quorum(), 14);
    }

    #[test]
    fn the_head_count_floor_is_n_minus_f_for_equal_weights() {
        for n in 1..=200u16 {
            let c = Committee::new(n);
            assert_eq!(u128::from(c.min_quorum_size()), c.quorum(), "n = {n}");
        }
        // One heavy member reaches the weight by itself, but a quorum also
        // needs n − f seats, so the floor is the seat quorum.
        let c = Committee::weighted(vec![1_000_000_000, 1_000, 1_000, 1_000]).expect("4");
        assert_eq!(c.min_quorum_size(), 3);
        // Three heavy members carry the weight; the seats still need five.
        let c = Committee::weighted(vec![1_000, 1_000, 1_000, 1_000, 1, 1, 1]).expect("7");
        assert_eq!((c.count_quorum(), c.min_quorum_size()), (5, 5));
    }

    #[test]
    fn a_committee_larger_than_a_validator_id_can_name_is_refused() {
        assert!(Committee::weighted(vec![1; usize::from(u16::MAX) + 1]).is_none());
    }

    /// Eric 2026-11-24 third round 5: a tombstone the floor holds in its seat
    /// has zero consensus weight. Under the dual quorum (third round 3) zero
    /// weight is also no seat: it keeps its index and leader slot, and its
    /// signature never stands in for a voter's, by weight or by head count.
    #[test]
    fn a_zero_weight_seat_is_seated_but_never_counted() {
        let c = Committee::weighted(vec![5, 5, 5, 0]).expect("4");
        assert_eq!(c.size(), 4, "the seat is held");
        assert_eq!(c.weight(3), 0);
        assert_eq!(c.voters(), 3, "the count rule's n is the three voters");
        assert_eq!((c.quorum(), c.count_quorum()), (11, 3));
        assert!(c.is_quorum([0, 1, 2]));
        assert!(!c.is_quorum([0, 1, 3]), "the held seat adds no weight");
        assert_eq!(c.voters_in([0, 1, 3, 3]), 2);
        assert_eq!(c.leader(6), Some(3), "it keeps its leader slot");
        // At the four-seat floor the held seat must not spend the one seat
        // fault: with it counted, one honest outage would halt the chain.
        let c = Committee::weighted(vec![5, 5, 5, 5, 0]).expect("5");
        assert_eq!((c.voters(), c.count_faults()), (4, 1));
        assert!(c.is_quorum([0, 1, 2]), "one of four voters may be down");
        // Validity needs f_n + 1 = 2 voters; weight alone is not enough, and
        // the held seat is not the second voter.
        let c = Committee::weighted(vec![20, 1, 1, 1, 0]).expect("5");
        assert!(c.weight_of([0, 4]) >= c.validity());
        assert!(
            !c.is_validity([0, 4]),
            "the held seat is not a second voter"
        );
        assert!(c.is_validity([0, 1]));
    }

    /// Third round 3 + 5 together (review of #707): with unequal stakes
    /// beside a held seat, two voters carry W − f by weight alone
    /// ([9000, 5000, 3000, 0]: {0, 1} and {0, 2} both clear 11,334 and share
    /// one voter). The seat rule, counted over voters, needs all three, and
    /// the held tombstone's signature never stands in for the missing one.
    #[test]
    fn unequal_stakes_beside_a_held_seat_need_every_voter() {
        let c = Committee::weighted(vec![9_000, 5_000, 3_000, 0]).expect("4");
        assert!(c.weight_of([0, 1]) >= c.quorum() && c.weight_of([0, 2]) >= c.quorum());
        assert_eq!((c.voters(), c.count_faults(), c.count_quorum()), (3, 0, 3));
        assert_eq!(c.min_quorum_size(), 3);
        for split in [&[0, 1][..], &[0, 2], &[0, 1, 3], &[0, 2, 3], &[1, 2, 3]] {
            assert!(!c.is_quorum(split.iter().copied()), "{split:?} is a quorum");
        }
        assert!(c.is_quorum([0, 1, 2]));
        assert!(!c.is_validity([3]), "the held seat alone supports nothing");
    }

    #[test]
    fn a_committee_with_no_weight_is_refused() {
        // Every set would be a quorum of zero.
        assert!(Committee::weighted(vec![0; 4]).is_none());
        assert!(
            Committee::weighted(Vec::new()).is_some(),
            "empty stays as it was"
        );
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
        let g = Certificate::genesis(&c);
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
        assert!(cert(vec![0, 1, 2]).is_well_formed(&c));
        assert!(!cert(vec![0, 1]).is_well_formed(&c));
        assert!(!cert(vec![0, 1, 1]).is_well_formed(&c));
        assert!(!cert(vec![0, 1, 9]).is_well_formed(&c));
        let mut unpaired = cert(vec![0, 1, 2]);
        unpaired.signatures.pop();
        assert!(!unpaired.is_well_formed(&c), "a vote without its signature");
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
