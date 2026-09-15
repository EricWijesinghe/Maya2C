//! Fuzzes the relay's untrusted decoders: the 56-byte header, raw Ethernet
//! frames off an AF_XDP ring, the kernel's verdict, and the user-space receiver.
//!
//! Before the AEAD, a relay datagram is whatever anyone on the network sent to
//! the relay port; before the XDP program, a frame is whatever reached the NIC.
//! Every decoder here must refuse or understand any input — never panic, never
//! allocate without bound.
//!
//! Two agreement properties beyond survival:
//!
//! - **The kernel and user space agree.** A datagram the XDP verdict would
//!   redirect is one `RelayHeader::split` accepts, and the reverse. If they
//!   disagreed, the kernel would either pass user space garbage it assumes was
//!   filtered, or drop honest chunks user space would have taken.
//! - **A decoded header re-encodes to the bytes it came from.** Otherwise two
//!   different datagrams could carry the same AEAD associated data.

#![no_main]

use std::time::Instant;

use libfuzzer_sys::fuzz_target;
use maya_ebpf_net::common::header::{HEADER_LEN, MAX_DATAGRAM_LEN, RelayHeader};
use maya_ebpf_net::common::packet::{self, Parsed};
use maya_ebpf_net::common::rate::RateLimit;
use maya_ebpf_net::common::verdict::{Verdict, judge};
use maya_ebpf_net::{KeyBook, Limits, RelayKey, RelayReceiver};

fuzz_target!(|data: &[u8]| {
    let split = RelayHeader::split(data);
    if let Ok((header, sealed)) = &split {
        assert_eq!(header.encode()[..], data[..HEADER_LEN], "a header changed across a round trip");
        assert_eq!(header.datagram_len(), data.len());
        assert_eq!(sealed.len(), data.len() - HEADER_LEN);
    }

    let verdict = judge(
        None,
        None,
        RateLimit::DEFAULT,
        0,
        data.first_chunk::<HEADER_LEN>(),
        data.len(),
    )
    .verdict;
    assert_eq!(
        verdict == Verdict::Redirect,
        split.is_ok(),
        "the kernel verdict and the user-space decoder disagree"
    );

    if let Parsed::Udp(view) = packet::parse(data) {
        assert!(view.payload.len() <= data.len());
    }

    let book = KeyBook::shared();
    book.write()
        .expect("fresh lock")
        .insert(0u8, RelayKey::from_bytes(&[0x42; 32]));
    let mut receiver = RelayReceiver::new(book, Limits::DEFAULT);
    let now = Instant::now();
    for datagram in data.chunks(MAX_DATAGRAM_LEN) {
        let _ = receiver.ingest(datagram, now);
    }
});
