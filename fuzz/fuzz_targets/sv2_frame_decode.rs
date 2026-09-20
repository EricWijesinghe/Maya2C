//! Fuzzes the Stratum V2 frame and message decoders.
//!
//! This is the pool's front door. Every other decoder in this suite is reached
//! by a peer that already completed a libp2p handshake; these bytes arrive from
//! whatever connected to the mining port, and the daemon is meant to hold
//! 50,000 of those at once. A panic here is not a rejected frame, it is the
//! pool.
//!
//! Two entry points, driven from one input so the fuzzer can reach both:
//!
//! | Target | Entry point |
//! |---|---|
//! | frame | `Frame::decode` |
//! | message | `Message::from_frame` |
//!
//! ## Properties asserted
//!
//! **Canonicality.** A frame that decodes must re-encode to the exact bytes it
//! came from. The header is fixed at six bytes and the payload length is
//! declared, so there is exactly one encoding of any frame — and a second
//! encoding of a `SubmitSharesStandard` would be a second share identity for
//! one piece of work, which is precisely what the duplicate check cannot see.
//!
//! **Length honesty.** A decoded frame's payload must be exactly the length its
//! header declared. This is the property that lets a stream reader trust the
//! header when sizing a buffer.
//!
//! **Message round-tripping.** A payload that parses into a [`Message`] must
//! re-encode to that same payload, and the `channel_msg` flag must survive.
//!
//! ArgonBlake is never called: verifying a share is a 32 MiB Argon2id pass, and
//! a fuzzer that manages ten executions per second is a job that looks like it
//! ran. Share *validation* is covered by the pool's own tests; this target is
//! about the bytes.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_stratum_v2::frame::MAX_PAYLOAD_LEN;
use maya_stratum_v2::{Frame, Message};

fuzz_target!(|data: &[u8]| {
    let Ok(frame) = Frame::decode(data) else {
        return;
    };

    assert_eq!(
        frame.payload.len(),
        frame.header.msg_length as usize,
        "decoded payload length disagrees with the header's declaration"
    );
    assert!(
        frame.payload.len() <= MAX_PAYLOAD_LEN,
        "a payload over the transport limit was accepted"
    );
    assert_eq!(
        frame.encode(),
        data,
        "non-canonical frame encoding accepted"
    );

    let Ok(message) = Message::from_frame(&frame) else {
        return;
    };

    assert_eq!(
        message.msg_type(),
        frame.header.msg_type,
        "message parsed under a type other than the one the frame declared"
    );
    assert_eq!(
        message.is_channel_msg(),
        frame.header.is_channel_msg(),
        "channel_msg flag disagreed with the parsed message and was not caught"
    );

    let payload = message
        .encode_payload()
        .expect("a message that decoded must re-encode");
    assert_eq!(
        payload, frame.payload,
        "non-canonical message encoding accepted"
    );
});
