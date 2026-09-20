//! The driver's half of the threat-intel registry: turning a rejected gossip
//! message into evidence, and remembering where peers connected from.
//!
//! Split from `node.rs` for size, as `guard.rs` and `relay.rs` were.
//!
//! The node emits evidence; it never submits it. Submitting needs a funded
//! chain key, and the node holds none — so an operator's tool, or a test, turns
//! [`NodeEvent::AttackEvidence`] into a signed `TxKind::AttestAttack`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::time::Instant;

use libp2p::PeerId;
use maya_threat_intel::{AttackAttestation, OffenceKind, SignedGossip, author_of_peer_id};

use super::{NodeDriver, NodeEvent};
use crate::network::peer_health::{Offence, Verdict};

/// Peers whose last connection address is remembered.
///
/// Remembered past disconnection on purpose: the peer guard disconnects an
/// offender at once, and a firewall worker that could only see *connected*
/// peers would never learn the address of the one it must block.
pub const ADDRESS_BOOK_CAPACITY: usize = 4_096;

/// The address of each peer's most recent connection, oldest evicted first.
#[derive(Default)]
pub(super) struct AddressBook {
    order: VecDeque<PeerId>,
    addresses: HashMap<PeerId, IpAddr>,
}

impl AddressBook {
    /// Records `peer`'s connection address, replacing any earlier one.
    pub(super) fn note(&mut self, peer: PeerId, ip: IpAddr) {
        if self.addresses.insert(peer, ip).is_some() {
            return;
        }
        self.order.push_back(peer);
        while self.order.len() > ADDRESS_BOOK_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.addresses.remove(&oldest);
            }
        }
    }

    /// Every remembered peer and address.
    pub(super) fn snapshot(&self) -> Vec<(PeerId, IpAddr)> {
        self.addresses
            .iter()
            .map(|(peer, ip)| (*peer, *ip))
            .collect()
    }
}

impl NodeDriver {
    /// Emits the message as evidence if the verdict was an offence a third
    /// party can re-check. `signature` is the author's, as the message carried
    /// it (`network::evidence_tap`).
    pub(super) fn attest(
        &mut self,
        verdict: Verdict,
        author: Option<PeerId>,
        sequence_number: Option<u64>,
        signature: &[u8],
        data: &[u8],
    ) {
        let Some(kind) = verdict.offence().and_then(attestable) else {
            return;
        };
        let (Some(author), Some(sequence_number)) = (author, sequence_number) else {
            return;
        };
        // An ed25519 signature is 64 bytes; any other key type cannot be attested.
        let Ok(signature) = <[u8; 64]>::try_from(signature) else {
            return;
        };
        // Every node identity is ed25519; any other key type cannot be attested.
        let Some(author_key) = author_of_peer_id(&author.to_bytes()) else {
            return;
        };
        let gossip = SignedGossip {
            author: author_key,
            sequence_number,
            data: data.to_vec(),
            signature,
        };
        // Over the evidence cap: the offence stands locally, but is too large
        // to put in front of every node.
        if let Ok(attestation) = AttackAttestation::new(kind, gossip) {
            self.emit(NodeEvent::AttackEvidence(Box::new(attestation)));
        }
    }
}

impl NodeDriver {
    /// Makes `peers` exactly the set refused for an on-chain conviction.
    ///
    /// A newly convicted peer is refused the way a quarantined one is: gossipsub
    /// drops what it relays or authored, and the block list closes its
    /// connections and refuses new ones. An acquitted peer is re-admitted only if
    /// the local peer guard is not holding it too — the two refusals are
    /// independent, and lifting one must not lift the other.
    ///
    /// Never this node itself: an indicator against its own key is a matter for
    /// its operator, not for it to disconnect from itself over.
    pub(super) fn enforce_convictions(&mut self, peers: HashSet<PeerId>) {
        let local = *self.swarm.local_peer_id();
        let peers: HashSet<PeerId> = peers.into_iter().filter(|peer| *peer != local).collect();
        let newly_convicted: Vec<PeerId> = peers.difference(&self.convicted).copied().collect();
        let acquitted: Vec<PeerId> = self.convicted.difference(&peers).copied().collect();
        let now = Instant::now();

        for peer in newly_convicted {
            let behaviour = self.swarm.behaviour_mut();
            behaviour.gossipsub.blacklist_peer(&peer);
            behaviour.gossipsub.remove_explicit_peer(&peer);
            behaviour.block_list.block_peer(peer);
            self.emit(NodeEvent::PeerConvicted(peer));
        }
        for peer in acquitted {
            if !self.health.is_quarantined(&peer, now) {
                let behaviour = self.swarm.behaviour_mut();
                behaviour.gossipsub.remove_blacklisted_peer(&peer);
                behaviour.block_list.unblock_peer(peer);
            }
            self.emit(NodeEvent::PeerAcquitted(peer));
        }
        self.convicted = peers;
    }
}

/// The offences whose frames a third party can re-check. A frame that fails to
/// *decode* is not one — see `maya_threat_intel::OffenceKind`.
const fn attestable(offence: Offence) -> Option<OffenceKind> {
    match offence {
        Offence::InvalidSignature => Some(OffenceKind::InvalidSignature),
        Offence::TxRootMismatch => Some(OffenceKind::TxRootMismatch),
        Offence::MalformedFrame | Offence::InvalidBlock | Offence::BadSyncResponse => None,
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn the_book_keeps_the_newest_address_and_evicts_the_oldest_peer() {
        let mut book = AddressBook::default();
        let first = PeerId::random();
        book.note(first, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
        book.note(first, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        assert_eq!(
            book.snapshot(),
            vec![(first, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)))]
        );

        for _ in 0..ADDRESS_BOOK_CAPACITY {
            book.note(PeerId::random(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        }
        assert_eq!(book.snapshot().len(), ADDRESS_BOOK_CAPACITY);
        assert!(book.snapshot().iter().all(|(peer, _)| *peer != first));
    }

    #[test]
    fn only_offences_a_third_party_can_recheck_are_attestable() {
        assert_eq!(
            attestable(Offence::InvalidSignature),
            Some(OffenceKind::InvalidSignature)
        );
        assert_eq!(
            attestable(Offence::TxRootMismatch),
            Some(OffenceKind::TxRootMismatch)
        );
        assert_eq!(attestable(Offence::MalformedFrame), None);
        assert_eq!(attestable(Offence::InvalidBlock), None);
        assert_eq!(attestable(Offence::BadSyncResponse), None);
    }
}
