//! Carries each inbound gossip message's author signature to the driver, so a
//! rejected message can become evidence.
//!
//! Gossipsub checks the signature in its codec — Strict mode, before the
//! behaviour sees the message — and then drops it: the [`Message`] handed to
//! the application has no signature field. This [`DataTransform`], the one hook
//! that sees the [`RawMessage`], puts the signature in front of the message
//! data, and the driver takes it off again with [`split`].
//!
//! In the message rather than in a table beside it, on purpose. A table keyed
//! by author and sequence number is shared state that a flood of validly signed
//! messages can evict before the driver claims an entry, which silently turns
//! evidence capture off. A signature riding inside its own message cannot be
//! evicted, is gone once the message is judged, and exists only for messages
//! gossipsub actually delivers.
//!
//! Nothing else changes. Gossipsub's default message id is the author plus the
//! sequence number, never the data, and forwarding uses the raw message, not
//! this one.

use std::io;

use libp2p::gossipsub::{DataTransform, Message, RawMessage, TopicHash};

/// Bytes of the signature-length prefix.
const LENGTH_BYTES: usize = 2;

/// The transform. Stateless.
#[derive(Clone, Copy, Debug, Default)]
pub struct EvidenceTap;

/// Splits delivered message data into the author's signature and the frame the
/// author published. `None` only for data that did not come through
/// [`EvidenceTap`].
#[must_use]
pub fn split(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let (length, rest) = data.split_first_chunk::<LENGTH_BYTES>()?;
    let length = usize::from(u16::from_le_bytes(*length));
    (rest.len() >= length).then(|| rest.split_at(length))
}

impl DataTransform for EvidenceTap {
    fn inbound_transform(&self, raw: RawMessage) -> Result<Message, io::Error> {
        // Strict mode refuses unsigned messages before this runs; an absent
        // signature is carried as an empty one rather than trusted to be
        // impossible. Any key type's signature fits in a `u16` length.
        let signature = raw.signature.as_deref().unwrap_or_default();
        let length = u16::try_from(signature.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "gossip signature too long"))?;
        let mut data = Vec::with_capacity(LENGTH_BYTES + signature.len() + raw.data.len());
        data.extend_from_slice(&length.to_le_bytes());
        data.extend_from_slice(signature);
        data.extend_from_slice(&raw.data);
        Ok(Message {
            source: raw.source,
            data,
            sequence_number: raw.sequence_number,
            topic: raw.topic,
        })
    }

    fn outbound_transform(&self, _topic: &TopicHash, data: Vec<u8>) -> Result<Vec<u8>, io::Error> {
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use libp2p::PeerId;
    use libp2p::gossipsub::IdentTopic;

    use super::*;

    fn raw(signature: Option<Vec<u8>>) -> RawMessage {
        RawMessage {
            source: Some(PeerId::random()),
            data: b"frame".to_vec(),
            sequence_number: Some(7),
            topic: IdentTopic::new("t").hash(),
            signature,
            key: None,
            validated: false,
        }
    }

    #[test]
    fn the_signature_rides_in_front_of_the_frame_and_splits_back_off() {
        let original = raw(Some(vec![9; 64]));
        let message = EvidenceTap
            .inbound_transform(original.clone())
            .expect("transform");
        assert_eq!(message.source, original.source);
        assert_eq!(message.sequence_number, original.sequence_number);
        let (signature, frame) = split(&message.data).expect("split");
        assert_eq!(signature, [9; 64].as_slice());
        assert_eq!(frame, b"frame");
    }

    #[test]
    fn an_unsigned_message_carries_an_empty_signature_and_short_data_does_not_split() {
        let message = EvidenceTap.inbound_transform(raw(None)).expect("transform");
        let (signature, frame) = split(&message.data).expect("split");
        assert!(signature.is_empty());
        assert_eq!(frame, b"frame");
        assert_eq!(split(&[5]), None);
        assert_eq!(split(&[4, 0, 1, 2]), None);
    }
}
