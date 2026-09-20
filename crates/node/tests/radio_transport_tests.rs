//! Two air-gapped nodes, a 30% lossy radio channel, and whether the headers
//! arrive.
//!
//! ## What "air-gapped" means here
//!
//! The two nodes share no socket, no filesystem, and no in-process handle. The
//! only thing between them is a [`Channel`] that moves byte vectors and throws
//! some of them away. A test that passed with a shared `Arc` would be testing
//! the simulation rather than the transport.
//!
//! ## Why the loss model lives here and not in `src/network/sim.rs`
//!
//! `sim.rs` wraps a libp2p byte *stream*. Dropping bytes from a stream is not
//! packet loss — it is corruption, and no real link does it. Radio loses whole
//! frames, and a frame only exists inside this transport, so the erasure
//! channel belongs where frames do.
//!
//! ## The loss pattern is seeded, not random
//!
//! A test that failed one run in twenty would be a test nobody trusts and
//! everybody reruns. Every channel here is driven by an explicit seed, so a
//! failure reproduces exactly — and the recovery property is asserted across
//! many seeds rather than once, because the interesting question is the tail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{BlockHeader, block::HEADER_LEN};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::Result;
use custom_l1_node::network::radio_gateway::{HEADERS_PER_OBJECT, RadioGateway, RadioLink};

use maya_radio_transport::duty::{Band, Settings};
use maya_radio_transport::frame::{Frame, Kind, NodeTag};
use maya_radio_transport::relay::{Bundle, DEFAULT_HOPS, RelayStore};

// ---------------------------------------------------------------------------
// the channel
// ---------------------------------------------------------------------------

/// A one-way radio channel that loses a fixed share of what crosses it.
///
/// Frames go in as bytes and come out as bytes, so everything a real link does
/// to a frame — including delivering it corrupted — can be modelled by editing
/// the bytes in flight. Nothing here shares state with a node.
struct Channel {
    /// Frames in flight, as the bytes a radio would put on air.
    air: Vec<Vec<u8>>,
    /// Percent of frames dropped.
    loss_percent: u32,
    /// The seeded generator, so a failure reproduces.
    state: u64,
    /// How many frames were offered and how many crossed.
    offered: usize,
    delivered: usize,
}

impl Channel {
    fn new(seed: u64, loss_percent: u32) -> Self {
        Self {
            air: Vec::new(),
            loss_percent,
            state: seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1),
            offered: 0,
            delivered: 0,
        }
    }

    /// The next draw, 0..100.
    fn roll(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 33) % 100
    }

    /// Offers a frame to the air.
    fn offer(&mut self, bytes: Vec<u8>) {
        self.offered += 1;
        if self.roll() < u64::from(self.loss_percent) {
            return;
        }
        self.delivered += 1;
        self.air.push(bytes);
    }

    /// Takes everything currently on the air.
    fn drain(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.air)
    }
}

/// One node's end of a channel: it transmits into `out` and receives from `in`.
struct Endpoint<'a> {
    outbound: &'a mut Channel,
    inbound: Vec<Vec<u8>>,
}

impl RadioLink for Endpoint<'_> {
    fn transmit(&mut self, frame: &Frame) -> Result<()> {
        self.outbound.offer(frame.encode());
        Ok(())
    }

    fn receive(&mut self) -> Result<Vec<Frame>> {
        // A frame that fails its checksum is dropped here rather than raised:
        // that is what a radio does, and a link that surfaced every corrupt
        // frame as an error would surface one constantly.
        Ok(std::mem::take(&mut self.inbound)
            .iter()
            .filter_map(|bytes| Frame::decode(bytes).ok())
            .collect())
    }
}

// ---------------------------------------------------------------------------
// fixtures
// ---------------------------------------------------------------------------

fn header(height: u64) -> BlockHeader {
    BlockHeader {
        prev_hash: [height as u8; 32],
        state_root: [(height + 1) as u8; 32],
        timestamp: 1_789_000_000 + height,
        nonce: height,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [(height + 2) as u8; 32],
    }
}

fn headers(count: usize) -> Vec<BlockHeader> {
    (0..count as u64).map(header).collect()
}

const SENDER: NodeTag = [0x0a, 0x01];
const RELAY: NodeTag = [0x0a, 0x02];
const RECEIVER: NodeTag = [0x0a, 0x03];

/// Runs one object across a lossy channel and returns what the far side got.
fn cross(seed: u64, loss: u32, settings: Settings, sent: &[BlockHeader]) -> Vec<BlockHeader> {
    let mut channel = Channel::new(seed, loss);

    let mut transmitter = RadioGateway::new(SENDER, Band::EU868, settings).expect("gateway");
    let mut receiver = RadioGateway::new(RECEIVER, Band::EU868, settings).expect("gateway");

    // The sender transmits across as many duty-cycle windows as it needs. Each
    // `broadcast_headers` call is one window's worth; the governor stops it.
    let mut now_micros = 0u64;
    for _ in 0..64 {
        let mut endpoint = Endpoint {
            outbound: &mut channel,
            inbound: Vec::new(),
        };
        let sent_frames = transmitter
            .broadcast_headers(&mut endpoint, 1, sent, now_micros)
            .expect("broadcast");
        if sent_frames == 0 {
            // The window is spent. An hour later the budget is back.
            now_micros += maya_radio_transport::duty::WINDOW_MICROS;
            continue;
        }
        now_micros += 1;

        let mut inbound = Endpoint {
            outbound: &mut Channel::new(0, 0),
            inbound: channel.drain(),
        };
        receiver.poll(&mut inbound, 0).expect("poll");
        let recovered = receiver.take_recovered();
        if !recovered.is_empty() {
            return recovered;
        }
    }
    Vec::new()
}

// ---------------------------------------------------------------------------
// 1. the channel itself
// ---------------------------------------------------------------------------

#[test]
fn the_channel_actually_loses_about_thirty_percent() {
    // A test harness that quietly delivered everything would make every test
    // below pass for the wrong reason.
    let mut channel = Channel::new(1, 30);
    for _ in 0..10_000 {
        channel.offer(vec![0u8; 8]);
    }
    let delivered = channel.delivered as f64 / channel.offered as f64;
    assert!(
        (0.66..0.74).contains(&delivered),
        "delivered {delivered:.3} of what was offered; the channel is not lossy"
    );
}

#[test]
fn a_corrupt_frame_never_reaches_a_gateway() {
    // The checksum is what stands between a shared band and a decoder fed
    // somebody else's bytes.
    let frame =
        Frame::broadcast(SENDER, Kind::Symbol, DEFAULT_HOPS, vec![1, 2, 3, 4]).expect("frame");
    let mut bytes = frame.encode();
    bytes[6] ^= 0x01;

    let mut endpoint = Endpoint {
        outbound: &mut Channel::new(0, 0),
        inbound: vec![bytes],
    };
    assert!(endpoint.receive().expect("receive").is_empty());
}

// ---------------------------------------------------------------------------
// 2. recovery across an air gap
// ---------------------------------------------------------------------------

#[test]
fn a_clean_channel_carries_every_header() {
    let sent = headers(HEADERS_PER_OBJECT);
    let got = cross(1, 0, Settings::EU868_FAST, &sent);
    assert_eq!(got, sent);
}

#[test]
fn thirty_percent_loss_still_recovers_every_header() {
    // The property the whole crate exists for. Air-gapped: the two gateways
    // share nothing but a channel that throws away three frames in ten.
    let sent = headers(HEADERS_PER_OBJECT);
    let got = cross(7, 30, Settings::EU868_FAST, &sent);
    assert_eq!(
        got.len(),
        sent.len(),
        "recovered {} of {}",
        got.len(),
        sent.len()
    );
    assert_eq!(got, sent);
}

#[test]
fn thirty_percent_loss_recovers_on_every_seed() {
    // Once is luck. The overhead constant is sized for the tail, so the tail is
    // what gets asserted.
    let sent = headers(HEADERS_PER_OBJECT);
    for seed in 0..25 {
        let got = cross(seed, 30, Settings::EU868_FAST, &sent);
        assert_eq!(got, sent, "seed {seed} lost headers");
    }
}

#[test]
fn recovery_holds_at_the_slowest_spreading_factor() {
    // SF12 is 26 bytes of payload per frame, so sixteen headers become 89
    // blocks and the object crosses many duty-cycle windows. The code does not
    // care; the clock does.
    let sent = headers(HEADERS_PER_OBJECT);
    let got = cross(3, 30, Settings::EU868_LONG, &sent);
    assert_eq!(got, sent);
}

#[test]
fn half_the_frames_lost_still_recovers_given_more_windows() {
    // Past what the overhead is sized for. A fountain code does not fail at
    // 50% — it takes longer, because the sender simply keeps emitting symbols.
    let sent = headers(HEADERS_PER_OBJECT);
    let got = cross(11, 50, Settings::EU868_FAST, &sent);
    assert_eq!(got, sent);
}

#[test]
fn a_recovered_header_is_byte_identical_to_the_one_sent() {
    // Not merely "a header" — the same header. A transport that delivered a
    // plausible-but-different header would be worse than one that delivered
    // nothing, because proof of work is checked over exactly these bytes.
    let sent = headers(HEADERS_PER_OBJECT);
    let got = cross(5, 30, Settings::EU868_FAST, &sent);
    for (received, original) in got.iter().zip(&sent) {
        assert_eq!(received.serialize(), original.serialize());
    }
}

// ---------------------------------------------------------------------------
// 3. the duty cycle is not negotiable
// ---------------------------------------------------------------------------

#[test]
fn a_gateway_stops_transmitting_when_the_budget_is_spent() {
    // The regulatory property. A gateway that kept going would make its
    // operator an unlicensed transmitter, and no amount of pending traffic
    // changes that.
    let mut channel = Channel::new(0, 0);
    let mut gateway =
        RadioGateway::new(SENDER, Band::EU868, Settings::EU868_LONG).expect("gateway");
    let sent = headers(HEADERS_PER_OBJECT);

    let mut endpoint = Endpoint {
        outbound: &mut channel,
        inbound: Vec::new(),
    };
    let frames = gateway
        .broadcast_headers(&mut endpoint, 1, &sent, 0)
        .expect("broadcast");

    // An hour of EU868 at SF12 is fourteen full frames, and the object needs
    // far more than that — so the governor, not the object, decides.
    assert!(frames > 0, "nothing was sent at all");
    assert!(
        frames <= 14,
        "sent {frames} frames in one window; the 1% budget allows at most 14"
    );
}

#[test]
fn an_object_larger_than_one_window_still_completes_over_several() {
    // The consequence of the test above: a header batch at SF12 takes hours,
    // and the transport has to be indifferent to that.
    let sent = headers(HEADERS_PER_OBJECT);
    let got = cross(2, 0, Settings::EU868_LONG, &sent);
    assert_eq!(got, sent);
}

// ---------------------------------------------------------------------------
// 4. store-and-forward across a node that is never online with either end
// ---------------------------------------------------------------------------

#[test]
fn a_bundle_crosses_two_hops_that_are_never_up_at_once() {
    // The premise of delay-tolerant networking, and the thing no routing
    // protocol can do: the sender and the receiver are never in contact, and
    // the relay is in contact with each of them at different times.
    let payload = header(42).serialize().to_vec();
    let bundle = Bundle::new(payload.clone(), 10_000, DEFAULT_HOPS).expect("bundle");

    // t=0: the sender hands it to the relay. The receiver is not listening.
    let mut relay = RelayStore::new();
    assert!(relay.accept(bundle, 0).expect("accept"));

    // t=5000: the relay meets the receiver. The sender is long gone.
    let outbound = relay.outbound(5_000);
    assert_eq!(outbound.len(), 1);

    let mut destination = RelayStore::new();
    assert!(
        destination
            .accept(outbound[0].clone(), 5_000)
            .expect("accept")
    );

    let delivered = destination.outbound(5_000);
    assert_eq!(delivered[0].payload, payload);
    // And the hop count records the journey, which is what stops it looping.
    assert_eq!(delivered[0].travelled, 2);
}

#[test]
fn a_relay_carries_a_payload_it_cannot_interpret() {
    // The consensus-neutrality rule, exercised: a relay that dropped what it
    // could not parse would be a relay whose idea of valid decides what the
    // network delivers.
    let mut relay = RelayStore::new();
    for payload in [vec![], vec![0xff; 3], vec![0x00; HEADER_LEN - 1]] {
        let bundle = Bundle::new(payload, 10_000, DEFAULT_HOPS).expect("bundle");
        assert!(relay.accept(bundle, 0).expect("accept"));
    }
    assert_eq!(relay.len(), 3);
}

#[test]
fn a_bundle_stops_after_its_hop_budget() {
    // Otherwise a mesh where every node rebroadcasts is a mesh that never goes
    // quiet — and on a 1% duty cycle, spectrum spent looping is spectrum no
    // real traffic gets.
    let mut carried = Bundle::new(vec![1, 2, 3], 10_000, DEFAULT_HOPS).expect("bundle");
    let mut hops = 0;
    while let Ok(next) = carried.forward() {
        carried = next;
        hops += 1;
        assert!(hops < 64, "the hop budget must run out");
    }
    assert_eq!(hops, usize::from(DEFAULT_HOPS - 1));
}

#[test]
fn a_gateway_relays_a_bundle_it_received_over_the_air() {
    // End to end through the frame layer rather than the store alone.
    let mut inbound = Channel::new(0, 0);
    let mut gateway = RadioGateway::new(RELAY, Band::EU868, Settings::EU868_FAST).expect("gateway");

    let bundle =
        Bundle::new(b"opaque to every relay".to_vec(), 10_000, DEFAULT_HOPS).expect("bundle");
    let frame =
        Frame::broadcast(SENDER, Kind::Bundle, DEFAULT_HOPS, bundle.encode()).expect("frame");

    let mut endpoint = Endpoint {
        outbound: &mut inbound,
        inbound: vec![frame.encode()],
    };
    gateway.poll(&mut endpoint, 0).expect("poll");
    assert_eq!(gateway.stored_bundles(), 1);

    let mut out = Channel::new(0, 0);
    let mut sender = Endpoint {
        outbound: &mut out,
        inbound: Vec::new(),
    };
    assert_eq!(gateway.relay(&mut sender, 0, 0).expect("relay"), 1);
}
