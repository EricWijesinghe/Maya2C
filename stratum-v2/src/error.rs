//! Errors raised while framing or parsing Stratum V2 messages.
//!
//! Every variant here is reachable from bytes a peer chose, so each one carries
//! enough detail to tell an operator *which* peer sent *what* nonsense without
//! having to reproduce it. None of them is a panic: a pool that aborts on a
//! malformed frame is a pool one connection can take down.

/// Everything that can go wrong decoding or encoding a Stratum V2 frame.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Sv2Error {
    /// A frame ended sooner than its own fields required.
    #[error("malformed frame: {0}")]
    Decode(String),

    /// A declared payload length exceeds what the transport beneath can carry.
    #[error("payload of {len} bytes exceeds the {max}-byte maximum")]
    PayloadTooLarge { len: usize, max: usize },

    /// The frame header named an extension this build does not implement.
    #[error("unknown extension type {extension_type:#06x}")]
    UnknownExtension { extension_type: u16 },

    /// The frame header named a message type this build does not implement.
    #[error("unknown message type {msg_type:#04x} in extension {extension_type:#06x}")]
    UnknownMessageType { extension_type: u16, msg_type: u8 },

    /// A string field exceeded the 255-byte ceiling of `STR0_255`.
    #[error("string field is {len} bytes, over the 255-byte maximum")]
    StringTooLong { len: usize },

    /// A string field was not valid UTF-8.
    #[error("string field is not valid UTF-8: {0}")]
    NotUtf8(String),

    /// A float field arrived as NaN or an infinity.
    ///
    /// Rejected rather than clamped: a hash rate of NaN would propagate through
    /// every vardiff calculation that touches it, and the comparison that would
    /// normally catch a bad value silently returns false for NaN.
    #[error("{field} must be a finite number")]
    NonFiniteFloat { field: &'static str },

    /// A channel message carried a `channel_msg` flag that disagreed with its
    /// message type.
    #[error(
        "message type {msg_type:#04x} {expected} a channel id, but the frame's \
         channel_msg bit says otherwise"
    )]
    ChannelFlagMismatch {
        msg_type: u8,
        expected: &'static str,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Sv2Error>;
