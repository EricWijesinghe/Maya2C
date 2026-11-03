//! Envelope version 2: every ciphertext, key and signature is written as a
//! CBOR byte string instead of an integer array, about half the size
//! (ADR-074, decision 4).
//!
//! Only the canonical encoding ([`crate::relay::Envelope::encode`]) uses
//! this. The libp2p transport keeps the version-1 shape: `cbor4ii` refuses a
//! byte string where a `Vec<u8>` is expected, so changing the transport
//! would break every peer that has not upgraded (see the ADR's deviations).
//! Decoding needs no mirror: `ciborium` reads both shapes into the same types.

use serde::Serialize;

use crate::Address;
use crate::relay::{Envelope, Payload};
use crate::session::{Handshake, Message};

/// The version-2 shape of [`Envelope`]. Borrows, so encoding copies nothing.
#[derive(Serialize)]
pub(crate) struct EnvelopeV2<'a> {
    to: &'a Address,
    expires_at: u64,
    payload: PayloadV2<'a>,
    stamp: u64,
    version: u8,
}

#[derive(Serialize)]
enum PayloadV2<'a> {
    Open {
        handshake: HandshakeV2<'a>,
        first: MessageV2<'a>,
    },
    Chat(MessageV2<'a>),
}

#[derive(Serialize)]
struct MessageV2<'a> {
    session: &'a [u8; 32],
    n: u64,
    ciphertext: &'a serde_bytes::Bytes,
}

#[derive(Serialize)]
struct HandshakeV2<'a> {
    initiator_key: &'a serde_bytes::Bytes,
    responder: &'a Address,
    prekey_epoch: u32,
    ciphertext: &'a serde_bytes::Bytes,
    sent_at: u64,
    signature: &'a serde_bytes::Bytes,
}

impl<'a> From<&'a Envelope> for EnvelopeV2<'a> {
    fn from(envelope: &'a Envelope) -> Self {
        Self {
            to: &envelope.to,
            expires_at: envelope.expires_at,
            payload: PayloadV2::from(&envelope.payload),
            stamp: envelope.stamp,
            version: envelope.version,
        }
    }
}

impl<'a> From<&'a Payload> for PayloadV2<'a> {
    fn from(payload: &'a Payload) -> Self {
        match payload {
            Payload::Open { handshake, first } => Self::Open {
                handshake: HandshakeV2::from(handshake),
                first: MessageV2::from(first),
            },
            Payload::Chat(message) => Self::Chat(MessageV2::from(message)),
        }
    }
}

impl<'a> From<&'a Message> for MessageV2<'a> {
    fn from(message: &'a Message) -> Self {
        Self {
            session: &message.session,
            n: message.n,
            ciphertext: serde_bytes::Bytes::new(&message.ciphertext),
        }
    }
}

impl<'a> From<&'a Handshake> for HandshakeV2<'a> {
    fn from(handshake: &'a Handshake) -> Self {
        Self {
            initiator_key: serde_bytes::Bytes::new(&handshake.initiator_key),
            responder: &handshake.responder,
            prekey_epoch: handshake.prekey_epoch,
            ciphertext: serde_bytes::Bytes::new(&handshake.ciphertext),
            sent_at: handshake.sent_at,
            signature: serde_bytes::Bytes::new(&handshake.signature),
        }
    }
}
