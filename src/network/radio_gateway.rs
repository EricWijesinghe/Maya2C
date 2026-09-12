//! The node side of the radio link: headers out, headers in, and a relay
//! queue in between.
//!
//! ## Why this is a gateway and not a libp2p transport
//!
//! The plan for this subsystem was a `TransportKind::Radio` beside `Memory` and
//! `Tcp`. Working through the link budget says it should not be, and the reason
//! is worth writing down rather than discovering later.
//!
//! A libp2p connection opens with multistream-select, a Noise handshake and a
//! yamux negotiation. Maya2C's Noise is ML-KEM-768: a 1,184-byte encapsulation
//! key and a 1,088-byte ciphertext, before any protocol negotiation and before
//! one byte of payload. At SF12 the link carries 26 bytes of fountain payload
//! per frame and each frame costs 2.47 seconds of airtime against a 1% duty
//! cycle — roughly 246 seconds of enforced silence apiece.
//!
//! That handshake is therefore **hours**, repeated per connection, to move a
//! 144-byte header. A transport that cannot complete its own handshake inside
//! the useful lifetime of what it carries is not a transport.
//!
//! So the radio path is a gateway: it takes headers from the chain, puts them
//! on the air as fountain symbols, takes symbols off the air, and hands
//! recovered headers back. No session, no negotiation, no per-peer state that
//! has to survive a week of intermittent contact. This is the shape every
//! working LoRa mesh converges on, and it is what the architecture vision meant
//! by "low-bandwidth header relay".
//!
//! ## A gateway decides nothing
//!
//! `docs/architecture-vision.md` §3: a transport may not change consensus. So
//! this module hands a recovered header to exactly the code a TCP peer's header
//! reaches, and there is no branch anywhere that asks how it arrived. The one
//! thing it may do is *refuse to carry* — airtime is finite and somebody has to
//! choose — and that choice is by height, which is public and not a judgement
//! about validity.

use maya_radio_transport::duty::{Band, DutyCycle, Settings};
use maya_radio_transport::fountain::{Decoder, Encoder, Symbol};
use maya_radio_transport::frame::{self, Frame, Kind, NodeTag};
use maya_radio_transport::relay::{Bundle, RelayStore};

use crate::core::BlockHeader;
use crate::core::block::HEADER_LEN;
use crate::error::{NodeError, Result};

/// How many headers travel as one fountain object.
///
/// Sixteen, which is 2,304 bytes — 89 blocks at SF12's 26-byte payload, or 12
/// at SF7's 197. Batching matters more than it looks: a fountain code amortises
/// its overhead across the object, so sixteen headers cost far less than
/// sixteen separate single-header encodes.
pub const HEADERS_PER_OBJECT: usize = 16;

/// A link this gateway can transmit on.
///
/// A trait rather than a serial port, so the tests can supply a channel that
/// drops 30% of what it carries — and so `serialport` stays out of the node's
/// default dependency graph. A real implementation is a few lines over a
/// `SerialPort`, and belongs wherever the hardware is configured.
pub trait RadioLink {
    /// Puts one frame on the air. Returns whether it was sent.
    ///
    /// # Errors
    ///
    /// Implementations return whatever their hardware does. The gateway treats
    /// any error as "this frame did not go", which is the same as loss and is
    /// handled the same way — by sending another symbol.
    fn transmit(&mut self, frame: &Frame) -> Result<()>;

    /// Takes whatever has arrived since the last call.
    ///
    /// # Errors
    ///
    /// As [`RadioLink::transmit`].
    fn receive(&mut self) -> Result<Vec<Frame>>;
}

/// The radio side of a node.
pub struct RadioGateway {
    tag: NodeTag,
    duty: DutyCycle,
    block_size: usize,
    store: RelayStore,
    /// Decodes in flight, by object id.
    decoding: Vec<(u32, Decoder)>,
    /// Objects already recovered, newest last.
    ///
    /// A fountain sender does not stop when a receiver has enough — it cannot,
    /// having no return path — so spare symbols keep arriving for an object
    /// that is already complete. Without this the gateway started a fresh
    /// decode for each of them and delivered the same headers again: sixteen
    /// headers arrived a hundred and twelve times in the first run of
    /// `a_clean_channel_carries_every_header`.
    ///
    /// It is also a bound on work. Symbols come from anyone with a
    /// transmitter, and a receiver that re-decoded on demand would do unbounded
    /// Gaussian elimination for the price of repeating somebody else's frames.
    completed: Vec<u32>,
    /// The next symbol index to emit, per object.
    ///
    /// A fountain code's whole advantage is that the next symbol is as good as
    /// the last, so a sender interrupted by the duty cycle should resume with
    /// *new* symbols rather than repeat the ones it managed. Without this the
    /// gateway re-sent its first window forever: at SF12 an object needs about
    /// twelve windows, and every one of them carried the same fourteen frames.
    emitting: Vec<(u32, u32)>,
    /// Headers recovered and not yet taken.
    recovered: Vec<BlockHeader>,
}

impl RadioGateway {
    /// A gateway for a band and link settings.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the spreading factor leaves no room for
    /// a fountain symbol — which no LoRa setting does, but a caller that
    /// invented one deserves an answer rather than a panic.
    pub fn new(tag: NodeTag, band: Band, settings: Settings) -> Result<Self> {
        let block_size = maya_radio_transport::block_size_for(settings.spreading_factor)
            .ok_or_else(|| {
                NodeError::Decode(format!(
                    "spreading factor {} leaves no room for a symbol",
                    settings.spreading_factor
                ))
            })?;
        Ok(Self {
            tag,
            duty: DutyCycle::new(band, settings),
            block_size,
            store: RelayStore::new(),
            decoding: Vec::new(),
            completed: Vec::new(),
            emitting: Vec::new(),
            recovered: Vec::new(),
        })
    }

    /// The payload one fountain symbol carries on this link.
    #[must_use]
    pub const fn block_size(&self) -> usize {
        self.block_size
    }

    /// Puts a run of headers on the air, as far as the duty cycle allows.
    ///
    /// Returns how many frames were actually transmitted. A caller that gets
    /// back fewer than it hoped has not failed: calling again in the next
    /// window resumes with symbols it has not sent yet, and a receiver does not
    /// care which ones those are.
    ///
    /// Zero means the budget is spent, not that the object is finished — there
    /// is no "finished" for a rateless code, only a receiver that has enough.
    /// The caller stops when it has reason to believe somebody did.
    ///
    /// # Errors
    ///
    /// Returns a link error, or [`NodeError::Decode`] if the headers do not
    /// encode — which for fixed-width headers means only that too many were
    /// handed in at once.
    pub fn broadcast_headers(
        &mut self,
        link: &mut impl RadioLink,
        object: u32,
        headers: &[BlockHeader],
        now_micros: u64,
    ) -> Result<usize> {
        let mut bytes = Vec::with_capacity(headers.len() * HEADER_LEN);
        for header in headers {
            bytes.extend_from_slice(&header.serialize());
        }

        let encoder = Encoder::new(object, &bytes, self.block_size)
            .map_err(|error| NodeError::Decode(format!("radio: {error}")))?;

        let cursor = match self.emitting.iter().position(|(id, _)| *id == object) {
            Some(position) => position,
            None => {
                self.emitting.push((object, 0));
                self.emitting.len() - 1
            }
        };

        let mut sent = 0;
        loop {
            let index = self.emitting[cursor].1;
            let payload = encoder.symbol(index).encode();
            let frame = Frame::broadcast(self.tag, Kind::Symbol, DEFAULT_HOPS, payload)
                .map_err(|error| NodeError::Decode(format!("radio: {error}")))?;
            let air = frame.encode();

            // The governor decides, not the caller. A gateway that transmitted
            // "just one more" would be the unlicensed transmitter the whole
            // module exists to avoid.
            if self
                .duty
                .reserve(air.len(), now_micros + sent as u64)
                .is_err()
            {
                break;
            }
            link.transmit(&frame)?;
            self.emitting[cursor].1 = index.wrapping_add(1);
            sent += 1;
        }
        Ok(sent)
    }

    /// Forgets an object's send cursor, so the next broadcast starts over.
    ///
    /// For the case where a receiver has confirmed it has the object, or the
    /// object is no longer worth sending. Nothing here polls for that — a
    /// gateway has no return path to poll on.
    pub fn finish_object(&mut self, object: u32) {
        self.emitting.retain(|(id, _)| *id != object);
    }

    /// Takes whatever arrived, and recovers any headers it completes.
    ///
    /// Returns how many headers were recovered by this call.
    ///
    /// # Errors
    ///
    /// Returns a link error. A frame that fails its checksum, names an unknown
    /// kind, or carries a symbol for an object this node is not decoding is
    /// **dropped silently** — that is the ordinary weather of a shared band,
    /// and a gateway that errored on it would error constantly.
    pub fn poll(&mut self, link: &mut impl RadioLink, now: u64) -> Result<usize> {
        let before = self.recovered.len();
        for frame in link.receive()? {
            match frame.kind {
                Kind::Symbol => self.absorb_symbol(&frame),
                Kind::Bundle => self.absorb_bundle(&frame, now),
                Kind::Beacon => {}
            }
        }
        Ok(self.recovered.len() - before)
    }

    /// Headers recovered so far, taken out of the gateway.
    ///
    /// Draining rather than borrowing: a header handed to the chain is the
    /// chain's, and a gateway that kept a copy would be a second place for the
    /// same header to be applied from.
    pub fn take_recovered(&mut self) -> Vec<BlockHeader> {
        std::mem::take(&mut self.recovered)
    }

    /// Bundles held for relay.
    #[must_use]
    pub fn stored_bundles(&self) -> usize {
        self.store.len()
    }

    /// Feeds one fountain symbol in.
    fn absorb_symbol(&mut self, frame: &Frame) {
        let Ok(symbol) = Symbol::decode(&frame.payload) else {
            return;
        };
        let object = symbol.object;

        if self.completed.contains(&object) {
            return;
        }
        if !self.decoding.iter().any(|(id, _)| *id == object) {
            let Ok(decoder) = Decoder::new(&symbol) else {
                return;
            };
            self.decoding.push((object, decoder));
        }
        let Some((_, decoder)) = self.decoding.iter_mut().find(|(id, _)| *id == object) else {
            return;
        };

        if let Ok(Some(bytes)) = decoder.absorb(&symbol) {
            self.decoding.retain(|(id, _)| *id != object);
            self.remember_completed(object);
            self.recover_headers(&bytes);
        }
    }

    /// Records an object as done, forgetting the oldest when the list is full.
    ///
    /// Bounded rather than unbounded: a node runs for years on a shared band,
    /// and a set of every object id ever heard is a leak with an attacker
    /// holding the tap. Forgetting the oldest costs a re-decode of something
    /// long finished, which is the cheap direction to be wrong in.
    fn remember_completed(&mut self, object: u32) {
        if self.completed.len() >= MAX_REMEMBERED_OBJECTS {
            self.completed.remove(0);
        }
        self.completed.push(object);
    }

    /// Splits a recovered object back into headers.
    ///
    /// A trailing partial header is dropped rather than padded. The object's
    /// length is exact, so a remainder means the sender and this node disagree
    /// about `HEADER_LEN` — which is a version mismatch, not a header.
    fn recover_headers(&mut self, bytes: &[u8]) {
        let (whole, _remainder) = bytes.as_chunks::<HEADER_LEN>();
        for chunk in whole {
            if let Ok(header) = BlockHeader::from_bytes(chunk) {
                self.recovered.push(header);
            }
        }
    }

    /// Takes custody of a relayed bundle.
    fn absorb_bundle(&mut self, frame: &Frame, now: u64) {
        let Ok(bundle) = Bundle::decode(&frame.payload) else {
            return;
        };
        // An expired or duplicate bundle is not an error — it is what a mesh
        // where every neighbour rebroadcasts looks like from the inside.
        let _ = self.store.accept(bundle, now);
    }

    /// Offers everything held onward, as far as the duty cycle allows.
    ///
    /// # Errors
    ///
    /// Returns a link error.
    pub fn relay(&mut self, link: &mut impl RadioLink, now: u64, now_micros: u64) -> Result<usize> {
        let mut sent = 0;
        for bundle in self.store.outbound(now) {
            let payload = bundle.encode();
            if payload.len() > frame::MAX_PAYLOAD {
                continue;
            }
            let Ok(out) = Frame::broadcast(self.tag, Kind::Bundle, bundle.hops, payload) else {
                continue;
            };
            let air = out.encode();
            if self
                .duty
                .reserve(air.len(), now_micros + sent as u64)
                .is_err()
            {
                break;
            }
            link.transmit(&out)?;
            sent += 1;
        }
        Ok(sent)
    }
}

/// How many finished objects a gateway remembers.
///
/// A few hundred windows of history, which at any spreading factor is far more
/// than the time a sender keeps emitting for one object.
const MAX_REMEMBERED_OBJECTS: usize = 256;

/// Hop budget a gateway stamps on what it originates.
const DEFAULT_HOPS: u8 = maya_radio_transport::relay::DEFAULT_HOPS;
