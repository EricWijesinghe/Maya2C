//! Fuzzes the radio decoders: frame framing, fountain symbols, and relay
//! bundles.
//!
//! These bytes have the weakest provenance of any input in this tree. An
//! archive comes from a store somebody configured; a transaction comes from a
//! peer that completed a post-quantum handshake; an ISO 20022 message comes
//! over a bank rail with a counterparty at the other end. A radio frame comes
//! from *anyone with a transmitter*, on a shared unlicensed band, with nothing
//! in front of the parser at all.
//!
//! So the property is total: any input is refused or understood, never a panic
//! and never an unbounded allocation.
//!
//! Two properties beyond survival:
//!
//! - **A decoded frame re-encodes to itself.** A frame that changed on the way
//!   through would be a relay that altered what it forwarded.
//! - **A bundle's digest is the digest of its payload.** It is recomputed on
//!   decode rather than trusted, because trusting it would let one sender claim
//!   another's dedup key and suppress it across the mesh.

#![no_main]

use libfuzzer_sys::fuzz_target;
use maya_radio_transport::fountain::{Decoder, Symbol};
use maya_radio_transport::frame::Frame;
use maya_radio_transport::relay::Bundle;

fuzz_target!(|data: &[u8]| {
    if let Ok(frame) = Frame::decode(data) {
        let re_encoded = frame.encode();
        let again = Frame::decode(&re_encoded).expect("a decoded frame must re-decode");
        assert_eq!(again, frame, "a frame changed across a round trip");

        // Whatever the frame carries, hand it to the layer that would get it.
        match frame.kind {
            maya_radio_transport::frame::Kind::Symbol => {
                if let Ok(symbol) = Symbol::decode(&frame.payload) {
                    assert_eq!(
                        Symbol::decode(&symbol.encode()).expect("re-decode"),
                        symbol,
                        "a symbol changed across a round trip"
                    );
                    // A decoder built from a symbol must survive being fed it.
                    if let Ok(mut decoder) = Decoder::new(&symbol) {
                        let _ = decoder.absorb(&symbol);
                    }
                }
            }
            maya_radio_transport::frame::Kind::Bundle => {
                if let Ok(bundle) = Bundle::decode(&frame.payload) {
                    let again = Bundle::decode(&bundle.encode()).expect("re-decode");
                    assert_eq!(again, bundle, "a bundle changed across a round trip");
                    assert_eq!(
                        blake3::hash(&bundle.payload).as_bytes()[..bundle.digest.len()],
                        bundle.digest[..],
                        "a bundle decoded with a digest that is not its payload's"
                    );
                }
            }
            maya_radio_transport::frame::Kind::Beacon => {}
        }
    }

    // The inner decoders on their own, over whatever the input spells. A frame
    // is not the only way these are reached — a relay hands a payload straight
    // across — so they get their own pass rather than only the one that arrives
    // wrapped in valid framing.
    if let Ok(symbol) = Symbol::decode(data) {
        let _ = Decoder::new(&symbol);
    }
    if let Ok(bundle) = Bundle::decode(data) {
        let _ = bundle.forward();
    }
});
