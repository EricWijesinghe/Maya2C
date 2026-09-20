//! Store-and-forward: carrying a message until somebody has the internet.
//!
//! ## The shape of the problem
//!
//! There is no path. A node in a valley hears two neighbours, one of whom walks
//! over a ridge each week, and somewhere past that is a node with IP transit.
//! No routing protocol converges on that, because there is no moment when the
//! links are simultaneously up. What works is the postal model: hold a message,
//! hand it to whoever you meet, and let it arrive by whatever chain of custody
//! happens.
//!
//! So this is delay-tolerant networking in the sense CCSDS BPv7 means it — a
//! bundle, a custodian, a lifetime — rather than a routing table.
//!
//! ## A relay never looks inside
//!
//! [`Bundle::payload`] is bytes. Nothing here parses it, validates it, or
//! decides anything by it. That is the constraint from
//! `docs/architecture-vision.md` §3, and this module is where it is easiest to
//! break: a relay that dropped "invalid" bundles would be a relay whose idea of
//! valid is now part of what the network delivers, and an attacker who learns
//! that rule gets to choose what propagates.
//!
//! The one thing a relay does judge is *cost*: hops and lifetime, both of which
//! are about the mesh's own resources and not about the message.
//!
//! ## Why dedup is by digest and not by identifier
//!
//! A bundle's id is chosen by whoever made it. Deduplicating on it would let
//! one sender claim an id and suppress somebody else's bundle for the whole
//! store's lifetime — a censorship primitive costing one frame. The digest is
//! over the payload, so two bundles collide only if they carry the same bytes,
//! which is the case dedup is for.
//!
//! ## Eviction is by expiry, then by hops travelled
//!
//! A full store drops the bundle closest to death, and among equals the one
//! that has travelled furthest — because that one has had the most chances to
//! reach its destination already, while a fresh bundle has had none. Dropping
//! the newest would make a busy relay a black hole for everything arriving
//! during the busy period.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::frame::NodeTag;

/// Bytes of a bundle's framing: digest, expiry, hops, and the payload length.
const BUNDLE_OVERHEAD: usize = 16 + 8 + 1 + 1 + 2;

/// Bytes of the payload digest kept for dedup.
///
/// Sixteen, truncated from BLAKE3. A collision means one bundle suppresses
/// another, so this is sized against a deliberate search rather than against
/// accident — 128 bits is far past what a transmitter-hour can reach.
const DIGEST_BYTES: usize = 16;

/// The most payload one bundle may carry.
///
/// A bundle rides in fountain symbols like anything else, so this is a bound on
/// what a relay stores rather than on what a frame holds.
pub const MAX_BUNDLE_PAYLOAD: usize = 8 * 1_024;

/// The most bundles a store holds before it evicts.
pub const DEFAULT_CAPACITY: usize = 512;

/// How far a bundle may travel, in hops.
///
/// Eight. A mesh this is for is a handful of nodes deep, and the cost of a hop
/// budget that is too generous is a message circulating after everyone who
/// wanted it has it — on a 1% duty cycle, that is spectrum nobody gets back.
pub const DEFAULT_HOPS: u8 = 8;

/// One message in transit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    /// BLAKE3 of the payload, truncated. The dedup key.
    pub digest: [u8; DIGEST_BYTES],
    /// When this stops being worth relaying, on the caller's clock.
    pub expiry: u64,
    /// Hops remaining.
    pub hops: u8,
    /// Hops already taken, for eviction order.
    pub travelled: u8,
    /// The bytes. Never inspected here.
    pub payload: Vec<u8>,
}

impl Bundle {
    /// A new bundle.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] for a payload past [`MAX_BUNDLE_PAYLOAD`].
    pub fn new(payload: Vec<u8>, expiry: u64, hops: u8) -> Result<Self> {
        if payload.len() > MAX_BUNDLE_PAYLOAD {
            return Err(Error::Oversized {
                what: "bundle payload",
                found: payload.len(),
                limit: MAX_BUNDLE_PAYLOAD,
            });
        }
        Ok(Self {
            digest: digest_of(&payload),
            expiry,
            hops,
            travelled: 0,
            payload,
        })
    }

    /// Whether this bundle is past its lifetime at `now`.
    #[must_use]
    pub const fn is_expired(&self, now: u64) -> bool {
        now >= self.expiry
    }

    /// This bundle as it leaves for the next hop.
    ///
    /// # Errors
    ///
    /// Returns [`Error::HopsExhausted`] when the budget is spent. That is not a
    /// fault — it is how a mesh stops a message circulating forever — so a
    /// caller should treat it as "keep it, do not forward it".
    pub fn forward(&self) -> Result<Self> {
        let hops = self.hops.checked_sub(1).ok_or(Error::HopsExhausted)?;
        if hops == 0 {
            return Err(Error::HopsExhausted);
        }
        Ok(Self {
            hops,
            travelled: self.travelled.saturating_add(1),
            ..self.clone()
        })
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + BUNDLE_OVERHEAD);
        out.extend_from_slice(&self.digest);
        out.extend_from_slice(&self.expiry.to_le_bytes());
        out.push(self.hops);
        out.push(self.travelled);
        out.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Reads a bundle.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a truncated bundle, a declared length
    /// that disagrees with what arrived, or a digest that is not the payload's.
    ///
    /// The digest is **recomputed**, never trusted. A relay that took the
    /// sender's word would let one sender pick another's dedup key and suppress
    /// it everywhere the bundle reached.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < BUNDLE_OVERHEAD {
            return Err(Error::Malformed(format!(
                "{} bytes is shorter than a bundle's framing",
                bytes.len()
            )));
        }
        let digest: [u8; DIGEST_BYTES] = bytes[..DIGEST_BYTES].try_into().expect("checked above");
        let expiry = u64::from_le_bytes(bytes[16..24].try_into().expect("8 bytes"));
        let hops = bytes[24];
        let travelled = bytes[25];
        let length = u16::from_le_bytes(bytes[26..28].try_into().expect("2 bytes")) as usize;

        let payload = &bytes[BUNDLE_OVERHEAD..];
        if payload.len() != length {
            return Err(Error::Malformed(format!(
                "bundle declares {length} payload bytes and carries {}",
                payload.len()
            )));
        }
        if digest_of(payload) != digest {
            return Err(Error::Malformed(
                "bundle digest is not the digest of its payload".into(),
            ));
        }
        Ok(Self {
            digest,
            expiry,
            hops,
            travelled,
            payload: payload.to_vec(),
        })
    }
}

/// BLAKE3 of a payload, truncated to the dedup key.
fn digest_of(payload: &[u8]) -> [u8; DIGEST_BYTES] {
    let full = blake3::hash(payload);
    let mut out = [0u8; DIGEST_BYTES];
    out.copy_from_slice(&full.as_bytes()[..DIGEST_BYTES]);
    out
}

/// What a node is holding for others.
#[derive(Clone, Debug)]
pub struct RelayStore {
    capacity: usize,
    held: HashMap<[u8; DIGEST_BYTES], Bundle>,
    /// Digests already seen, so a bundle that has been delivered and dropped is
    /// not accepted back from a neighbour that still has it.
    seen: HashMap<[u8; DIGEST_BYTES], u64>,
}

impl RelayStore {
    /// A store of the default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// A store holding at most `capacity` bundles.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            held: HashMap::new(),
            seen: HashMap::new(),
        }
    }

    /// How many bundles are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// Whether this payload has been seen, held or delivered.
    #[must_use]
    pub fn has_seen(&self, digest: &[u8; DIGEST_BYTES]) -> bool {
        self.seen.contains_key(digest) || self.held.contains_key(digest)
    }

    /// Takes custody of a bundle.
    ///
    /// Returns `false` when the bundle is a duplicate — which is the normal
    /// case in a mesh where every neighbour rebroadcasts, and not an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Expired`] for a bundle past its lifetime, and
    /// [`Error::StoreFull`] when the store is full of bundles that are all
    /// fresher and less travelled than this one.
    pub fn accept(&mut self, bundle: Bundle, now: u64) -> Result<bool> {
        if bundle.is_expired(now) {
            return Err(Error::Expired {
                expiry: bundle.expiry,
                now,
            });
        }
        if self.has_seen(&bundle.digest) {
            return Ok(false);
        }

        self.expire(now);
        if self.held.len() >= self.capacity {
            self.evict_for(&bundle)?;
        }
        self.held.insert(bundle.digest, bundle);
        Ok(true)
    }

    /// Every bundle worth forwarding at `now`, each already decremented.
    ///
    /// A bundle whose hop budget is spent stays held — it may still be handed
    /// to a node that asks for it directly — but it is not offered onward.
    #[must_use]
    pub fn outbound(&self, now: u64) -> Vec<Bundle> {
        let mut out: Vec<Bundle> = self
            .held
            .values()
            .filter(|bundle| !bundle.is_expired(now))
            .filter_map(|bundle| bundle.forward().ok())
            .collect();
        // Oldest first: a bundle nearest expiry has the least time left to find
        // a path. Sorted rather than left to the map's order, because HashMap
        // iteration order differs between runs and a relay that reordered its
        // queue every restart would be hard to reason about in the field.
        out.sort_by_key(|bundle| (bundle.expiry, bundle.digest));
        out
    }

    /// Marks a payload delivered: dropped, and refused if it comes back.
    pub fn delivered(&mut self, digest: &[u8; DIGEST_BYTES], now: u64) {
        self.held.remove(digest);
        self.seen.insert(*digest, now);
    }

    /// Drops everything past its lifetime.
    ///
    /// The `seen` set is aged on the same pass: a digest remembered forever
    /// would be a slow leak on a node that runs for years, and the reason to
    /// remember one at all — stopping a delivered bundle coming back — expires
    /// with the bundle.
    pub fn expire(&mut self, now: u64) {
        self.held.retain(|_, bundle| !bundle.is_expired(now));
        self.seen
            .retain(|_, at| now.saturating_sub(*at) < SEEN_LIFETIME);
    }

    /// Makes room for `incoming`, or refuses.
    fn evict_for(&mut self, incoming: &Bundle) -> Result<()> {
        // Closest to death first, then furthest travelled. A fresh bundle has
        // had no chance to find a path; one that has crossed six hops has had
        // six.
        let victim = self
            .held
            .values()
            .min_by_key(|bundle| (bundle.expiry, std::cmp::Reverse(bundle.travelled)))
            .map(|bundle| (bundle.digest, bundle.expiry, bundle.travelled));

        match victim {
            Some((digest, expiry, travelled))
                if (expiry, std::cmp::Reverse(travelled))
                    < (incoming.expiry, std::cmp::Reverse(incoming.travelled)) =>
            {
                self.held.remove(&digest);
                Ok(())
            }
            _ => Err(Error::StoreFull {
                held: self.held.len(),
            }),
        }
    }
}

impl Default for RelayStore {
    fn default() -> Self {
        Self::new()
    }
}

/// How long a delivered digest is remembered.
///
/// Long enough that a neighbour still holding the bundle does not hand it
/// straight back, short enough that the set does not grow without bound on a
/// node that runs for years.
const SEEN_LIFETIME: u64 = 24 * 60 * 60;

/// A node's view of the mesh: who it has heard from, and when.
///
/// Deliberately not a routing table. There is no path to compute — the whole
/// premise is that links are never simultaneously up — so what a node keeps is
/// a record of contact, used to decide whether transmitting now is likely to
/// reach anyone at all.
#[derive(Clone, Debug, Default)]
pub struct Neighbours {
    heard: HashMap<NodeTag, u64>,
}

impl Neighbours {
    /// An empty view.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a frame from a neighbour.
    pub fn heard(&mut self, tag: NodeTag, now: u64) {
        self.heard.insert(tag, now);
    }

    /// Neighbours heard within `window` of `now`.
    #[must_use]
    pub fn active(&self, now: u64, window: u64) -> Vec<NodeTag> {
        let mut tags: Vec<NodeTag> = self
            .heard
            .iter()
            .filter(|(_, at)| now.saturating_sub(**at) <= window)
            .map(|(tag, _)| *tag)
            .collect();
        tags.sort_unstable();
        tags
    }

    /// Whether anybody has been heard recently enough to be worth transmitting
    /// to.
    #[must_use]
    pub fn any_active(&self, now: u64, window: u64) -> bool {
        !self.active(now, window).is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn bundle(payload: &[u8], expiry: u64) -> Bundle {
        Bundle::new(payload.to_vec(), expiry, DEFAULT_HOPS).expect("within bounds")
    }

    #[test]
    fn a_bundle_survives_its_wire_format() {
        let original = bundle(b"a block header, as far as this crate knows", 1_000);
        assert_eq!(
            Bundle::decode(&original.encode()).expect("decode"),
            original
        );
    }

    #[test]
    fn a_bundles_digest_is_recomputed_and_not_trusted() {
        // Trusting it would let one sender claim another's dedup key and
        // suppress that bundle everywhere it reached — censorship for the price
        // of one frame.
        let mut encoded = bundle(b"payload", 1_000).encode();
        encoded[0] ^= 0xff;
        assert!(Bundle::decode(&encoded).is_err());
    }

    #[test]
    fn two_bundles_with_the_same_payload_are_one_bundle() {
        let mut store = RelayStore::new();
        assert!(store.accept(bundle(b"same", 1_000), 0).expect("accept"));
        assert!(!store.accept(bundle(b"same", 1_000), 0).expect("duplicate"));
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_relay_never_inspects_a_payload() {
        // Bytes that are not a header, not a transaction, not anything. The
        // store must carry them exactly as it carries a real one: a relay whose
        // idea of "valid" filtered traffic would be a relay an attacker can
        // teach what to drop.
        let mut store = RelayStore::new();
        for payload in [b"".as_slice(), b"\x00\xff\x00".as_slice(), &[0xab; 4_000]] {
            let bundle = Bundle::new(payload.to_vec(), 1_000, DEFAULT_HOPS).expect("any bytes");
            assert!(store.accept(bundle, 0).expect("accept"));
        }
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn a_bundle_loses_a_hop_at_each_relay_and_stops() {
        let mut carried = bundle(b"walking", 1_000);
        let mut hops = 0;
        while let Ok(next) = carried.forward() {
            carried = next;
            hops += 1;
            assert!(hops < 100, "the hop budget must run out");
        }
        assert_eq!(hops, DEFAULT_HOPS - 1);
        assert_eq!(carried.travelled, DEFAULT_HOPS - 1);
    }

    #[test]
    fn an_expired_bundle_is_refused_and_a_held_one_is_dropped() {
        let mut store = RelayStore::new();
        store.accept(bundle(b"fresh", 100), 0).expect("accept");
        assert_eq!(store.len(), 1);

        store.expire(100);
        assert!(store.is_empty());

        assert!(matches!(
            store.accept(bundle(b"stale", 100), 200),
            Err(Error::Expired { .. })
        ));
    }

    #[test]
    fn a_full_store_drops_the_bundle_closest_to_death() {
        let mut store = RelayStore::with_capacity(2);
        store.accept(bundle(b"dies first", 100), 0).expect("accept");
        store.accept(bundle(b"dies later", 900), 0).expect("accept");

        // A bundle with more life than the soonest-expiring one displaces it.
        assert!(store.accept(bundle(b"fresh", 500), 0).expect("accept"));
        assert_eq!(store.len(), 2);
        assert!(!store.has_seen(&digest_of(b"dies first")));
        assert!(store.has_seen(&digest_of(b"dies later")));

        // And one with less life than anything held is refused rather than
        // displacing something with a better chance.
        assert!(matches!(
            store.accept(bundle(b"nearly dead", 10), 0),
            Err(Error::StoreFull { .. })
        ));
    }

    #[test]
    fn among_equals_the_furthest_travelled_is_dropped() {
        // It has had the most chances to arrive; a fresh bundle has had none.
        let mut store = RelayStore::with_capacity(1);
        let mut travelled = bundle(b"been around", 500);
        travelled.travelled = 6;
        store.accept(travelled, 0).expect("accept");

        assert!(store.accept(bundle(b"just made", 500), 0).expect("accept"));
        assert!(!store.has_seen(&digest_of(b"been around")));
    }

    #[test]
    fn a_delivered_bundle_does_not_come_back() {
        // Every neighbour rebroadcasts, so without this a delivered bundle
        // returns from the next node that still holds it, forever.
        let mut store = RelayStore::new();
        let bundle = bundle(b"delivered", 1_000);
        let digest = bundle.digest;
        store.accept(bundle.clone(), 0).expect("accept");
        store.delivered(&digest, 0);

        assert!(store.is_empty());
        assert!(!store.accept(bundle, 1).expect("refused as seen"));
    }

    #[test]
    fn the_seen_set_does_not_grow_forever() {
        // A node runs for years. A digest remembered past the lifetime of the
        // bundle it describes is a slow leak with nothing to show for it.
        let mut store = RelayStore::new();
        store.delivered(&digest_of(b"old"), 0);
        store.expire(SEEN_LIFETIME);
        assert!(!store.has_seen(&digest_of(b"old")));
    }

    #[test]
    fn the_outbound_queue_is_the_same_order_on_every_run() {
        // HashMap iteration order differs between runs. A relay whose queue
        // reordered on every restart would be very hard to reason about from a
        // hilltop with one radio and no shell.
        let mut store = RelayStore::new();
        for (index, expiry) in [(b"a", 300u64), (b"b", 100), (b"c", 200)] {
            store.accept(bundle(index, expiry), 0).expect("accept");
        }
        let first: Vec<u64> = store.outbound(0).iter().map(|b| b.expiry).collect();
        let second: Vec<u64> = store.outbound(0).iter().map(|b| b.expiry).collect();
        assert_eq!(first, second);
        assert_eq!(first, vec![100, 200, 300], "nearest expiry leaves first");
    }

    #[test]
    fn an_oversized_payload_is_refused() {
        assert!(Bundle::new(vec![0; MAX_BUNDLE_PAYLOAD + 1], 1_000, DEFAULT_HOPS).is_err());
    }

    #[test]
    fn neighbours_are_a_record_of_contact_and_not_a_route() {
        let mut mesh = Neighbours::new();
        mesh.heard([1, 1], 0);
        mesh.heard([2, 2], 500);

        assert_eq!(mesh.active(500, 600), vec![[1, 1], [2, 2]]);
        // The first neighbour has not been heard from inside the window.
        assert_eq!(mesh.active(600, 200), vec![[2, 2]]);
        assert!(!mesh.any_active(10_000, 100));
    }
}
