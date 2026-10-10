//! Every message this build speaks, and the mapping between message types and
//! frames.
//!
//! Message numbers follow the Stratum V2 specification slot for slot, even
//! where a field set had to change (see [`mining`]). Keeping the numbering
//! means anyone holding the spec can read a hex dump of this protocol; renaming
//! the slots would have bought nothing and cost that.
//!
//! Job Declaration messages (`0x50`–`0x5b`) are deliberately absent. They are
//! Phase 6 work, and an empty variant that decodes into nothing is worse than
//! an honest [`Sv2Error::UnknownMessageType`].

pub mod common;
pub mod mining;

use crate::codec::{Reader, Writer};
use crate::error::{Result, Sv2Error};
use crate::frame::{Frame, MAYA_EXTENSION_TYPE};

pub use common::{Protocol, SetupConnection, SetupConnectionError, SetupConnectionSuccess};
pub use mining::{
    CloseChannel, NewMiningJob, OpenMiningChannelError, OpenStandardMiningChannel,
    OpenStandardMiningChannelSuccess, SetNewPrevHash, SetNonceRange, SetTarget, SubmitSharesError,
    SubmitSharesStandard, SubmitSharesSuccess, SubmitWorkerTelemetry, UpdateChannel, error_codes,
};

/// Message type bytes, matching the Stratum V2 specification's assignments.
pub mod msg_type {
    /// [`super::SetupConnection`].
    pub const SETUP_CONNECTION: u8 = 0x00;
    /// [`super::SetupConnectionSuccess`].
    pub const SETUP_CONNECTION_SUCCESS: u8 = 0x01;
    /// [`super::SetupConnectionError`].
    pub const SETUP_CONNECTION_ERROR: u8 = 0x02;
    /// [`super::OpenStandardMiningChannel`].
    pub const OPEN_STANDARD_MINING_CHANNEL: u8 = 0x10;
    /// [`super::OpenStandardMiningChannelSuccess`].
    pub const OPEN_STANDARD_MINING_CHANNEL_SUCCESS: u8 = 0x11;
    /// [`super::OpenMiningChannelError`].
    pub const OPEN_MINING_CHANNEL_ERROR: u8 = 0x14;
    /// [`super::UpdateChannel`].
    pub const UPDATE_CHANNEL: u8 = 0x15;
    /// [`super::CloseChannel`].
    pub const CLOSE_CHANNEL: u8 = 0x17;
    /// [`super::SetNonceRange`], in SV2's `SetExtranoncePrefix` slot.
    pub const SET_NONCE_RANGE: u8 = 0x18;
    /// [`super::SubmitSharesStandard`].
    pub const SUBMIT_SHARES_STANDARD: u8 = 0x1a;
    /// [`super::SubmitSharesSuccess`].
    pub const SUBMIT_SHARES_SUCCESS: u8 = 0x1c;
    /// [`super::SubmitSharesError`].
    pub const SUBMIT_SHARES_ERROR: u8 = 0x1d;
    /// [`super::NewMiningJob`].
    pub const NEW_MINING_JOB: u8 = 0x1e;
    /// [`super::SetNewPrevHash`].
    pub const SET_NEW_PREV_HASH: u8 = 0x20;
    /// [`super::SetTarget`].
    pub const SET_TARGET: u8 = 0x21;
    /// [`super::SubmitWorkerTelemetry`], in SV2's `SubmitSolution` slot.
    ///
    /// That slot belongs to job declaration, which this build does not
    /// implement and which `Maya2C`'s header leaves nothing to declare.
    pub const SUBMIT_WORKER_TELEMETRY: u8 = 0x22;
}

/// Any message this build can send or receive.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// See [`SetupConnection`].
    SetupConnection(SetupConnection),
    /// See [`SetupConnectionSuccess`].
    SetupConnectionSuccess(SetupConnectionSuccess),
    /// See [`SetupConnectionError`].
    SetupConnectionError(SetupConnectionError),
    /// See [`OpenStandardMiningChannel`].
    OpenStandardMiningChannel(OpenStandardMiningChannel),
    /// See [`OpenStandardMiningChannelSuccess`].
    OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccess),
    /// See [`OpenMiningChannelError`].
    OpenMiningChannelError(OpenMiningChannelError),
    /// See [`UpdateChannel`].
    UpdateChannel(UpdateChannel),
    /// See [`CloseChannel`].
    CloseChannel(CloseChannel),
    /// See [`SetNonceRange`].
    SetNonceRange(SetNonceRange),
    /// See [`SubmitSharesStandard`].
    SubmitSharesStandard(SubmitSharesStandard),
    /// See [`SubmitSharesSuccess`].
    SubmitSharesSuccess(SubmitSharesSuccess),
    /// See [`SubmitSharesError`].
    SubmitSharesError(SubmitSharesError),
    /// See [`NewMiningJob`].
    NewMiningJob(NewMiningJob),
    /// See [`SetNewPrevHash`].
    SetNewPrevHash(SetNewPrevHash),
    /// See [`SetTarget`].
    SetTarget(SetTarget),
    /// See [`SubmitWorkerTelemetry`].
    SubmitWorkerTelemetry(SubmitWorkerTelemetry),
}

impl Message {
    /// The wire message type.
    #[must_use]
    pub fn msg_type(&self) -> u8 {
        use msg_type as t;
        match self {
            Self::SetupConnection(_) => t::SETUP_CONNECTION,
            Self::SetupConnectionSuccess(_) => t::SETUP_CONNECTION_SUCCESS,
            Self::SetupConnectionError(_) => t::SETUP_CONNECTION_ERROR,
            Self::OpenStandardMiningChannel(_) => t::OPEN_STANDARD_MINING_CHANNEL,
            Self::OpenStandardMiningChannelSuccess(_) => t::OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
            Self::OpenMiningChannelError(_) => t::OPEN_MINING_CHANNEL_ERROR,
            Self::UpdateChannel(_) => t::UPDATE_CHANNEL,
            Self::CloseChannel(_) => t::CLOSE_CHANNEL,
            Self::SetNonceRange(_) => t::SET_NONCE_RANGE,
            Self::SubmitSharesStandard(_) => t::SUBMIT_SHARES_STANDARD,
            Self::SubmitSharesSuccess(_) => t::SUBMIT_SHARES_SUCCESS,
            Self::SubmitSharesError(_) => t::SUBMIT_SHARES_ERROR,
            Self::NewMiningJob(_) => t::NEW_MINING_JOB,
            Self::SetNewPrevHash(_) => t::SET_NEW_PREV_HASH,
            Self::SetTarget(_) => t::SET_TARGET,
            Self::SubmitWorkerTelemetry(_) => t::SUBMIT_WORKER_TELEMETRY,
        }
    }

    /// Whether this message's payload begins with a channel id.
    ///
    /// Setup messages predate any channel, and the three that carry a
    /// `request_id` are answering a channel that does not exist yet.
    #[must_use]
    pub fn is_channel_msg(&self) -> bool {
        !matches!(
            self,
            Self::SetupConnection(_)
                | Self::SetupConnectionSuccess(_)
                | Self::SetupConnectionError(_)
                | Self::OpenStandardMiningChannel(_)
                | Self::OpenStandardMiningChannelSuccess(_)
                | Self::OpenMiningChannelError(_)
        )
    }

    /// Encodes the payload, without the frame header.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] if a text field is over-long.
    pub fn encode_payload(&self) -> Result<Vec<u8>> {
        let mut writer = Writer::new();
        match self {
            Self::SetupConnection(m) => m.encode(&mut writer)?,
            Self::SetupConnectionSuccess(m) => m.encode(&mut writer),
            Self::SetupConnectionError(m) => m.encode(&mut writer)?,
            Self::OpenStandardMiningChannel(m) => m.encode(&mut writer)?,
            Self::OpenStandardMiningChannelSuccess(m) => m.encode(&mut writer),
            Self::OpenMiningChannelError(m) => m.encode(&mut writer)?,
            Self::UpdateChannel(m) => m.encode(&mut writer),
            Self::CloseChannel(m) => m.encode(&mut writer)?,
            Self::SetNonceRange(m) => m.encode(&mut writer),
            Self::SubmitSharesStandard(m) => m.encode(&mut writer),
            Self::SubmitSharesSuccess(m) => m.encode(&mut writer),
            Self::SubmitSharesError(m) => m.encode(&mut writer)?,
            Self::NewMiningJob(m) => m.encode(&mut writer),
            Self::SetNewPrevHash(m) => m.encode(&mut writer),
            Self::SetTarget(m) => m.encode(&mut writer),
            Self::SubmitWorkerTelemetry(m) => m.encode(&mut writer),
        }
        Ok(writer.finish())
    }

    /// Wraps this message in a frame.
    ///
    /// # Errors
    ///
    /// Propagates encoding failures and [`Sv2Error::PayloadTooLarge`].
    pub fn to_frame(&self) -> Result<Frame> {
        Frame::new(
            self.msg_type(),
            self.is_channel_msg(),
            self.encode_payload()?,
        )
    }

    /// Parses a payload of the given type.
    ///
    /// Every arm calls `finish()`, so a payload with trailing bytes is refused.
    /// That matters most for [`SubmitSharesStandard`]: two encodings of one
    /// share would be two share identities for one piece of work, and the
    /// duplicate check keys on the decoded fields.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::UnknownMessageType`] for an unrecognised type, or a
    /// decoding failure from the message body.
    pub fn decode_payload(msg_type: u8, payload: &[u8]) -> Result<Self> {
        // Full path: the `msg_type` parameter shadows the module of the same
        // name, which is worth the one line to keep the parameter named after
        // the wire field it carries.
        use crate::messages::msg_type as t;

        // Each arm decodes then finishes, so trailing bytes are rejected
        // uniformly rather than per message.
        macro_rules! parse {
            ($ty:path, $variant:path) => {{
                let mut reader = Reader::new(payload);
                let message = <$ty>::decode(&mut reader)?;
                reader.finish()?;
                $variant(message)
            }};
        }

        Ok(match msg_type {
            t::SETUP_CONNECTION => parse!(SetupConnection, Self::SetupConnection),
            t::SETUP_CONNECTION_SUCCESS => {
                parse!(SetupConnectionSuccess, Self::SetupConnectionSuccess)
            }
            t::SETUP_CONNECTION_ERROR => parse!(SetupConnectionError, Self::SetupConnectionError),
            t::OPEN_STANDARD_MINING_CHANNEL => {
                parse!(OpenStandardMiningChannel, Self::OpenStandardMiningChannel)
            }
            t::OPEN_STANDARD_MINING_CHANNEL_SUCCESS => parse!(
                OpenStandardMiningChannelSuccess,
                Self::OpenStandardMiningChannelSuccess
            ),
            t::OPEN_MINING_CHANNEL_ERROR => {
                parse!(OpenMiningChannelError, Self::OpenMiningChannelError)
            }
            t::UPDATE_CHANNEL => parse!(UpdateChannel, Self::UpdateChannel),
            t::CLOSE_CHANNEL => parse!(CloseChannel, Self::CloseChannel),
            t::SET_NONCE_RANGE => parse!(SetNonceRange, Self::SetNonceRange),
            t::SUBMIT_SHARES_STANDARD => parse!(SubmitSharesStandard, Self::SubmitSharesStandard),
            t::SUBMIT_SHARES_SUCCESS => parse!(SubmitSharesSuccess, Self::SubmitSharesSuccess),
            t::SUBMIT_SHARES_ERROR => parse!(SubmitSharesError, Self::SubmitSharesError),
            t::NEW_MINING_JOB => parse!(NewMiningJob, Self::NewMiningJob),
            t::SET_NEW_PREV_HASH => parse!(SetNewPrevHash, Self::SetNewPrevHash),
            t::SET_TARGET => parse!(SetTarget, Self::SetTarget),
            t::SUBMIT_WORKER_TELEMETRY => {
                parse!(SubmitWorkerTelemetry, Self::SubmitWorkerTelemetry)
            }
            other => {
                return Err(Sv2Error::UnknownMessageType {
                    extension_type: MAYA_EXTENSION_TYPE,
                    msg_type: other,
                });
            }
        })
    }

    /// Parses a complete frame.
    ///
    /// Checks the extension id and the `channel_msg` flag before looking at the
    /// payload. The flag check is not pedantry: a frame whose flag disagrees
    /// with its type was built by something that does not understand the
    /// protocol, and reading its payload as though it did is how a router ends
    /// up dispatching a share to the wrong channel.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::UnknownExtension`], [`Sv2Error::ChannelFlagMismatch`],
    /// [`Sv2Error::UnknownMessageType`], or a decoding failure.
    pub fn from_frame(frame: &Frame) -> Result<Self> {
        if frame.header.extension() != MAYA_EXTENSION_TYPE {
            return Err(Sv2Error::UnknownExtension {
                extension_type: frame.header.extension(),
            });
        }

        let message = Self::decode_payload(frame.header.msg_type, &frame.payload)?;

        if message.is_channel_msg() != frame.header.is_channel_msg() {
            return Err(Sv2Error::ChannelFlagMismatch {
                msg_type: frame.header.msg_type,
                expected: if message.is_channel_msg() {
                    "carries"
                } else {
                    "does not carry"
                },
            });
        }

        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// One of every message, for the round-trip sweep.
    fn every_message() -> Vec<Message> {
        vec![
            Message::SetupConnection(SetupConnection {
                protocol: Protocol::Mining,
                min_version: 2,
                max_version: 2,
                flags: 0b1010,
                endpoint_host: "pool.maya.example".to_string(),
                endpoint_port: 3333,
                vendor: "Maya".to_string(),
                hardware_version: "rev-2".to_string(),
                firmware: "0.1.0".to_string(),
                device_id: "farm-07.rig-3".to_string(),
            }),
            Message::SetupConnectionSuccess(SetupConnectionSuccess {
                used_version: 2,
                flags: 0,
            }),
            Message::SetupConnectionError(SetupConnectionError {
                flags: 1,
                error_code: "unsupported-feature-flags".to_string(),
            }),
            Message::OpenStandardMiningChannel(OpenStandardMiningChannel {
                request_id: 7,
                user_identity: "farm-07".to_string(),
                nominal_hash_rate: 40.0,
                max_target: [0xFF; 32],
            }),
            Message::OpenStandardMiningChannelSuccess(OpenStandardMiningChannelSuccess {
                request_id: 7,
                channel_id: 1,
                target: [0x0F; 32],
                nonce_min: 0,
                nonce_max: 1 << 40,
            }),
            Message::OpenMiningChannelError(OpenMiningChannelError {
                request_id: 7,
                error_code: "max-target-out-of-range".to_string(),
            }),
            Message::UpdateChannel(UpdateChannel {
                channel_id: 1,
                nominal_hash_rate: 80.5,
                maximum_target: [0x0F; 32],
            }),
            Message::CloseChannel(CloseChannel {
                channel_id: 1,
                reason_code: "shutting-down".to_string(),
            }),
            Message::SetNonceRange(SetNonceRange {
                channel_id: 1,
                nonce_min: 1 << 40,
                nonce_max: 1 << 41,
            }),
            Message::SubmitSharesStandard(SubmitSharesStandard {
                channel_id: 1,
                sequence_number: 42,
                job_id: 9,
                nonce: (1 << 40) + 12345,
                ntime: 1_800_000_000,
            }),
            Message::SubmitSharesSuccess(SubmitSharesSuccess {
                channel_id: 1,
                last_sequence_number: 42,
                new_submits_accepted_count: 12,
                new_shares_sum: 4_096,
            }),
            Message::SubmitSharesError(SubmitSharesError {
                channel_id: 1,
                sequence_number: 43,
                error_code: error_codes::STALE_SHARE.to_string(),
            }),
            Message::NewMiningJob(NewMiningJob {
                channel_id: 1,
                job_id: 9,
                state_root: [0xAB; 32],
                timestamp: 1_800_000_000,
                tx_root: [0xEF; 32],
            }),
            Message::SetNewPrevHash(SetNewPrevHash {
                channel_id: 1,
                job_id: 9,
                prev_hash: [0xCD; 32],
                min_ntime: 1_800_000_000,
                target: [0x00; 32],
            }),
            Message::SetTarget(SetTarget {
                channel_id: 1,
                maximum_target: [0x0F; 32],
            }),
            Message::SubmitWorkerTelemetry(SubmitWorkerTelemetry {
                channel_id: 1,
                power_milliwatts: 1_450_000,
                // Negative on purpose: an immersion rig runs below zero, and a
                // round trip that silently clamped it would still pass a test
                // written with a positive temperature.
                temperature_millicelsius: -5_250,
                fan_percent: 80,
                sample_millis: 10_000,
            }),
        ]
    }

    #[test]
    fn every_message_round_trips_through_a_frame() {
        for message in every_message() {
            let frame = message.to_frame().expect("encoding must succeed");
            let bytes = frame.encode();
            let decoded_frame = Frame::decode(&bytes).expect("frame must decode");
            let decoded = Message::from_frame(&decoded_frame).expect("message must decode");

            assert_eq!(decoded, message, "round trip changed {message:?}");
        }
    }

    #[test]
    fn every_message_type_is_distinct() {
        // A duplicated constant would silently route one message into another's
        // decoder, which for two same-length payloads produces a plausible
        // wrong value rather than an error.
        let mut seen = Vec::new();
        for message in every_message() {
            let ty = message.msg_type();
            assert!(!seen.contains(&ty), "message type {ty:#04x} used twice");
            seen.push(ty);
        }
    }

    #[test]
    fn the_channel_flag_matches_the_message_type() {
        for message in every_message() {
            let frame = message.to_frame().unwrap();
            assert_eq!(
                frame.header.is_channel_msg(),
                message.is_channel_msg(),
                "flag disagrees with {message:?}"
            );
        }
    }

    #[test]
    fn a_frame_with_the_wrong_channel_flag_is_refused() {
        let message = Message::SubmitSharesStandard(SubmitSharesStandard {
            channel_id: 1,
            sequence_number: 1,
            job_id: 1,
            nonce: 1,
            ntime: 1,
        });
        let payload = message.encode_payload().unwrap();

        // Same payload, same type, flag cleared.
        let forged = Frame::new(message.msg_type(), false, payload).unwrap();

        assert!(matches!(
            Message::from_frame(&forged),
            Err(Sv2Error::ChannelFlagMismatch { .. })
        ));
    }

    #[test]
    fn an_unknown_extension_is_refused() {
        let mut frame = Message::SetTarget(SetTarget {
            channel_id: 1,
            maximum_target: [0; 32],
        })
        .to_frame()
        .unwrap();

        // Extension 0x0000 is reserved for the real SV2 mining protocol. A
        // stock SV2 client's frames must not be parsed as Maya messages.
        frame.header.extension_type = 0x8000;

        assert!(matches!(
            Message::from_frame(&frame),
            Err(Sv2Error::UnknownExtension { .. })
        ));
    }

    #[test]
    fn an_unimplemented_message_type_is_refused_rather_than_ignored() {
        // 0x57 is SV2's DeclareMiningJob: Phase 6 work, not silently dropped.
        assert!(matches!(
            Message::decode_payload(0x57, &[]),
            Err(Sv2Error::UnknownMessageType { msg_type: 0x57, .. })
        ));
    }

    #[test]
    fn a_payload_with_trailing_bytes_is_refused() {
        let message = Message::SetTarget(SetTarget {
            channel_id: 1,
            maximum_target: [0x0F; 32],
        });
        let mut payload = message.encode_payload().unwrap();
        payload.push(0);

        assert!(Message::decode_payload(message.msg_type(), &payload).is_err());
    }

    #[test]
    fn a_truncated_payload_is_refused_for_every_message() {
        for message in every_message() {
            let payload = message.encode_payload().unwrap();
            for cut in 0..payload.len() {
                assert!(
                    Message::decode_payload(message.msg_type(), &payload[..cut]).is_err(),
                    "{message:?} decoded from {cut} of {} bytes",
                    payload.len()
                );
            }
        }
    }

    #[test]
    fn decoding_arbitrary_payloads_never_panics() {
        for msg_type in 0..=0x25u8 {
            for len in 0..80usize {
                for seed in [0u8, 0x5A, 0xFF] {
                    let payload: Vec<u8> = (0..len)
                        .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
                        .collect();
                    let _ = Message::decode_payload(msg_type, &payload);
                }
            }
        }
    }
}
