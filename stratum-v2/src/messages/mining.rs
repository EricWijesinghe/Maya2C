//! The mining sub-protocol, adapted to Maya2C's header.
//!
//! ## What changed from the specification, and why
//!
//! Stratum V2's mining messages are shaped around a Bitcoin header. Maya2C's is
//! 112 bytes of `prev_hash ‖ state_root ‖ timestamp ‖ nonce ‖ difficulty_target`
//! (`src/core/block.rs:33-41`) — no version field, no merkle root, no compact
//! `nbits`, and no coinbase to hide an extranonce in. Four adaptations follow.
//! Message numbering still matches SV2 slot for slot, so the correspondence
//! stays readable.
//!
//! | SV2 field | Here | Why |
//! |---|---|---|
//! | `merkle_root` in `NewMiningJob` | `state_root` | The header commits to post-execution state, not to a transaction tree (`src/state/db.rs:626-634`) |
//! | `nbits` (U32 compact) in `SetNewPrevHash` | `target` (32 bytes) | Maya2C targets are full 256-bit values compared bytewise (`src/crypto/pow.rs:16-18`); there is no compact form to pack into |
//! | `version` in jobs and shares | *absent* | The header has no version field to roll |
//! | `SetExtranoncePrefix` | [`SetNonceRange`] | No coinbase means no extranonce; see below |
//! | `ntime` (U32) | `timestamp` (U64) | The header's timestamp is a `u64` |
//!
//! ## Nonce ranges instead of extranonces
//!
//! In Bitcoin, each miner gets a distinct search space by varying the coinbase
//! extranonce. Maya2C has no coinbase, so the only field a miner may vary is
//! the 8-byte `nonce` — and with tens of thousands of connections on one job,
//! every one of them would otherwise start at zero and walk the same path.
//!
//! Each channel is therefore assigned a disjoint half-open nonce range at open
//! time, the way `src/consensus/miner.rs:6-10` already partitions across
//! threads. Two channels cannot collide by construction rather than by luck,
//! duplicate detection becomes exact, and a submitted nonce outside a channel's
//! range is rejected without hashing anything — which, at 25 ms per ArgonBlake
//! verification, is the cheapest rejection the pool has.

use crate::codec::{Reader, Writer};
use crate::error::{Result, Sv2Error};

/// Opens a channel: the miner states who it is and how fast it is.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenStandardMiningChannel {
    /// Correlates the response; the channel id does not exist yet.
    pub request_id: u32,
    /// Worker identity. The payout ledger keys on this, so it is not optional
    /// and it is not truncated.
    pub user_identity: String,
    /// Hashes per second the miner claims. A starting point for vardiff, never
    /// trusted: the pool measures the arrival rate and corrects.
    pub nominal_hash_rate: f32,
    /// Easiest target the miner will accept, in header byte order.
    pub max_target: [u8; 32],
}

impl OpenStandardMiningChannel {
    /// Appends this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] for an over-long identity.
    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u32(self.request_id);
        writer.write_str(&self.user_identity)?;
        writer.write_f32(self.nominal_hash_rate);
        writer.write_bytes32(&self.max_target);
        Ok(())
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures, including a non-finite hash rate.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            request_id: reader.read_u32()?,
            user_identity: reader.read_str()?,
            nominal_hash_rate: reader.read_f32("nominal_hash_rate")?,
            max_target: reader.read_bytes32()?,
        })
    }
}

/// The channel is open, here is its id, its target, and its nonce range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenStandardMiningChannelSuccess {
    /// Echoes the request.
    pub request_id: u32,
    /// Identifier every later channel message carries.
    pub channel_id: u32,
    /// Share target for this channel, in header byte order.
    pub target: [u8; 32],
    /// First nonce this channel owns.
    pub nonce_min: u64,
    /// One past the last nonce this channel owns.
    pub nonce_max: u64,
}

impl OpenStandardMiningChannelSuccess {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.request_id);
        writer.write_u32(self.channel_id);
        writer.write_bytes32(&self.target);
        writer.write_u64(self.nonce_min);
        writer.write_u64(self.nonce_max);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if the range is empty or inverted. An
    /// inverted range would leave a channel unable to submit anything the pool
    /// would accept, and the miner would have no way to tell why.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let message = Self {
            request_id: reader.read_u32()?,
            channel_id: reader.read_u32()?,
            target: reader.read_bytes32()?,
            nonce_min: reader.read_u64()?,
            nonce_max: reader.read_u64()?,
        };
        if message.nonce_min >= message.nonce_max {
            return Err(Sv2Error::Decode(format!(
                "empty nonce range [{}, {})",
                message.nonce_min, message.nonce_max
            )));
        }
        Ok(message)
    }

    /// Whether `nonce` falls inside this channel's range.
    #[must_use]
    pub fn contains(&self, nonce: u64) -> bool {
        (self.nonce_min..self.nonce_max).contains(&nonce)
    }
}

/// The channel could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenMiningChannelError {
    /// Echoes the request.
    pub request_id: u32,
    /// Machine-readable reason.
    pub error_code: String,
}

impl OpenMiningChannelError {
    /// Appends this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] for an over-long error code.
    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u32(self.request_id);
        writer.write_str(&self.error_code)
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            request_id: reader.read_u32()?,
            error_code: reader.read_str()?,
        })
    }
}

/// The miner revises its claimed hash rate or its target ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpdateChannel {
    /// Channel being updated.
    pub channel_id: u32,
    /// Revised hash rate.
    pub nominal_hash_rate: f32,
    /// Revised easiest acceptable target.
    pub maximum_target: [u8; 32],
}

impl UpdateChannel {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_f32(self.nominal_hash_rate);
        writer.write_bytes32(&self.maximum_target);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures, including a non-finite hash rate.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            nominal_hash_rate: reader.read_f32("nominal_hash_rate")?,
            maximum_target: reader.read_bytes32()?,
        })
    }
}

/// Either side closes a channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseChannel {
    /// Channel being closed.
    pub channel_id: u32,
    /// Machine-readable reason.
    pub reason_code: String,
}

impl CloseChannel {
    /// Appends this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] for an over-long reason code.
    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u32(self.channel_id);
        writer.write_str(&self.reason_code)
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            reason_code: reader.read_str()?,
        })
    }
}

/// Reassigns a channel's nonce range. Occupies SV2's `SetExtranoncePrefix`
/// slot; see the module docs for why an extranonce is not available here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetNonceRange {
    /// Channel being reassigned.
    pub channel_id: u32,
    /// First nonce this channel owns.
    pub nonce_min: u64,
    /// One past the last nonce this channel owns.
    pub nonce_max: u64,
}

impl SetNonceRange {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_u64(self.nonce_min);
        writer.write_u64(self.nonce_max);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] for an empty or inverted range.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let message = Self {
            channel_id: reader.read_u32()?,
            nonce_min: reader.read_u64()?,
            nonce_max: reader.read_u64()?,
        };
        if message.nonce_min >= message.nonce_max {
            return Err(Sv2Error::Decode(format!(
                "empty nonce range [{}, {})",
                message.nonce_min, message.nonce_max
            )));
        }
        Ok(message)
    }

    /// Whether `nonce` falls inside this range.
    #[must_use]
    pub fn contains(&self, nonce: u64) -> bool {
        (self.nonce_min..self.nonce_max).contains(&nonce)
    }
}

/// A new job: the two header fields the pool controls per template.
///
/// Together with the channel's most recent [`SetNewPrevHash`], this is
/// everything needed to build the 112-byte header except the nonce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NewMiningJob {
    /// Channel this job is for.
    pub channel_id: u32,
    /// Identifies the job in later share submissions.
    pub job_id: u32,
    /// Post-execution state root the header must commit to. Replaces SV2's
    /// `merkle_root`.
    pub state_root: [u8; 32],
    /// Header timestamp.
    pub timestamp: u64,
}

impl NewMiningJob {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_u32(self.job_id);
        writer.write_bytes32(&self.state_root);
        writer.write_u64(self.timestamp);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            job_id: reader.read_u32()?,
            state_root: reader.read_bytes32()?,
            timestamp: reader.read_u64()?,
        })
    }
}

/// The chain tip moved: every job before this one is stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetNewPrevHash {
    /// Channel being told.
    pub channel_id: u32,
    /// The first job valid on the new tip.
    pub job_id: u32,
    /// New parent block. Header bytes `[0..32]`.
    pub prev_hash: [u8; 32],
    /// Earliest header timestamp the pool will accept.
    pub min_ntime: u64,
    /// The block target the chain requires. Replaces SV2's `nbits`, and is not
    /// the channel's share target — see [`SetTarget`].
    pub target: [u8; 32],
}

impl SetNewPrevHash {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_u32(self.job_id);
        writer.write_bytes32(&self.prev_hash);
        writer.write_u64(self.min_ntime);
        writer.write_bytes32(&self.target);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            job_id: reader.read_u32()?,
            prev_hash: reader.read_bytes32()?,
            min_ntime: reader.read_u64()?,
            target: reader.read_bytes32()?,
        })
    }
}

/// Vardiff: the pool moves a channel's share target.
///
/// This is the pool's load governor, not a courtesy. Every accepted share costs
/// a 25 ms ArgonBlake verification, so the aggregate share rate — not the
/// connection count — is what decides whether the pool keeps up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetTarget {
    /// Channel being retargeted.
    pub channel_id: u32,
    /// Easiest digest the pool will now credit, in header byte order.
    pub maximum_target: [u8; 32],
}

impl SetTarget {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_bytes32(&self.maximum_target);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            maximum_target: reader.read_bytes32()?,
        })
    }
}

/// A submitted share.
///
/// Carries no `version` — the header has no version field — and its `ntime` is
/// a `u64` to match the header's timestamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitSharesStandard {
    /// Channel submitting.
    pub channel_id: u32,
    /// Monotonic per channel. What `SubmitSharesSuccess` acknowledges in bulk.
    pub sequence_number: u32,
    /// Job this share claims to solve.
    pub job_id: u32,
    /// The nonce found. Must lie in the channel's assigned range.
    pub nonce: u64,
    /// Header timestamp used.
    pub ntime: u64,
}

impl SubmitSharesStandard {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_u32(self.sequence_number);
        writer.write_u32(self.job_id);
        writer.write_u64(self.nonce);
        writer.write_u64(self.ntime);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            sequence_number: reader.read_u32()?,
            job_id: reader.read_u32()?,
            nonce: reader.read_u64()?,
            ntime: reader.read_u64()?,
        })
    }
}

/// Acknowledges every share up to `last_sequence_number`.
///
/// Batched rather than per-share on purpose: at 50,000 connections an
/// acknowledgement per submission would double the pool's write syscalls to
/// carry no information the miner did not already have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitSharesSuccess {
    /// Channel being acknowledged.
    pub channel_id: u32,
    /// Highest sequence number covered.
    pub last_sequence_number: u32,
    /// Shares accepted since the previous acknowledgement.
    pub new_submits_accepted_count: u32,
    /// Total share weight credited since the previous acknowledgement.
    pub new_shares_sum: u64,
}

impl SubmitSharesSuccess {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_u32(self.last_sequence_number);
        writer.write_u32(self.new_submits_accepted_count);
        writer.write_u64(self.new_shares_sum);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            last_sequence_number: reader.read_u32()?,
            new_submits_accepted_count: reader.read_u32()?,
            new_shares_sum: reader.read_u64()?,
        })
    }
}

/// One share was refused. Names the sequence number so the miner can tell which.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmitSharesError {
    /// Channel that submitted.
    pub channel_id: u32,
    /// Sequence number of the refused share.
    pub sequence_number: u32,
    /// Machine-readable reason.
    pub error_code: String,
}

impl SubmitSharesError {
    /// Appends this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] for an over-long error code.
    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u32(self.channel_id);
        writer.write_u32(self.sequence_number);
        writer.write_str(&self.error_code)
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            channel_id: reader.read_u32()?,
            sequence_number: reader.read_u32()?,
            error_code: reader.read_str()?,
        })
    }
}

/// Rig health a miner reports about itself.
///
/// ## Why this message exists at all
///
/// Stratum V2 has no slot for it: the specification's mining sub-protocol
/// carries only what the pool needs in order to hand out work and credit
/// shares, and a rig's power draw is neither. Operators want it anyway, and the
/// two places to put it are here or a second, separately authenticated HTTP
/// endpoint beside the mining port. This channel is already authenticated and
/// already identifies the worker, so a second endpoint would buy nothing and
/// cost a second way in.
///
/// It takes SV2's `SubmitSolution` slot (`0x22`), which this build does not use
/// — Maya2C has no job-declaration protocol, so no solution is ever submitted
/// separately from a share.
///
/// ## This data is self-reported and unverifiable
///
/// Nothing here is checked, and nothing here can be checked: a rig that claims
/// 40 watts while drawing 4,000 is indistinguishable from an efficient one. The
/// pool therefore exports these under `_reported_` metric names and **never**
/// lets them reach payout arithmetic. Treat it as a dashboard convenience that
/// happens to arrive over an authenticated channel, not as measurement.
///
/// ## Units
///
/// Milli-units throughout, because the alternative is a float and a float is a
/// NaN waiting to reach a comparison ([`crate::codec::Reader::read_f32`]
/// explains what that costs). Integers here cannot be non-finite, so there is
/// nothing to reject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubmitWorkerTelemetry {
    /// Channel reporting.
    pub channel_id: u32,
    /// Wall power draw, in milliwatts.
    pub power_milliwatts: u64,
    /// Hottest reported sensor, in millidegrees Celsius. Signed: immersion and
    /// sub-ambient rigs run below zero.
    pub temperature_millicelsius: i32,
    /// Fan duty cycle, 0–100. Values above 100 are refused on decode rather
    /// than clamped, because a rig reporting 300% is a rig whose telemetry is
    /// wrong in ways a clamp would hide.
    pub fan_percent: u8,
    /// How long the reported sample covers, in milliseconds.
    ///
    /// Power is an average over an interval; without the interval a dashboard
    /// cannot tell a 10-second average from an instantaneous spike, and cannot
    /// integrate to energy at all.
    pub sample_millis: u64,
}

/// Largest fan duty cycle a rig may report.
const MAX_FAN_PERCENT: u8 = 100;

impl SubmitWorkerTelemetry {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u32(self.channel_id);
        writer.write_u64(self.power_milliwatts);
        writer.write_i32(self.temperature_millicelsius);
        writer.write_u8(self.fan_percent);
        writer.write_u64(self.sample_millis);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] for a fan duty cycle above 100 or a
    /// zero-length sample window, and propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        let message = Self {
            channel_id: reader.read_u32()?,
            power_milliwatts: reader.read_u64()?,
            temperature_millicelsius: reader.read_i32()?,
            fan_percent: reader.read_u8()?,
            sample_millis: reader.read_u64()?,
        };

        if message.fan_percent > MAX_FAN_PERCENT {
            return Err(Sv2Error::Decode(format!(
                "fan_percent {} is above {MAX_FAN_PERCENT}",
                message.fan_percent
            )));
        }

        // A zero-length window makes the average it carries undefined, and a
        // consumer that divides by it gets an infinity — the same failure mode
        // `read_f32` exists to prevent, arriving by a different route.
        if message.sample_millis == 0 {
            return Err(Sv2Error::Decode(
                "sample_millis must be non-zero".to_string(),
            ));
        }

        Ok(message)
    }
}

/// Error codes this crate emits, from the SV2 vocabulary where one fits.
pub mod error_codes {
    /// The submitted nonce was outside the channel's assigned range.
    pub const NONCE_OUT_OF_RANGE: &str = "nonce-out-of-range";
    /// The job id is unknown or has been retired.
    pub const STALE_SHARE: &str = "stale-share";
    /// This exact share was already submitted.
    pub const DUPLICATE_SHARE: &str = "duplicate-share";
    /// The digest did not meet the channel's target.
    pub const DIFFICULTY_TOO_LOW: &str = "difficulty-too-low";
    /// The channel id is unknown on this connection.
    pub const UNKNOWN_CHANNEL: &str = "unknown-channel-id";
    /// The header timestamp was before `min_ntime` or implausibly far ahead.
    pub const INVALID_NTIME: &str = "invalid-ntime";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes a telemetry message and decodes it back.
    fn telemetry_round_trip(message: SubmitWorkerTelemetry) -> Result<SubmitWorkerTelemetry> {
        let mut writer = Writer::new();
        message.encode(&mut writer);
        let bytes = writer.finish();
        SubmitWorkerTelemetry::decode(&mut Reader::new(&bytes))
    }

    /// A telemetry sample that should decode cleanly.
    fn plausible_telemetry() -> SubmitWorkerTelemetry {
        SubmitWorkerTelemetry {
            channel_id: 1,
            power_milliwatts: 1_450_000,
            temperature_millicelsius: 62_500,
            fan_percent: 80,
            sample_millis: 10_000,
        }
    }

    #[test]
    fn a_sub_zero_temperature_survives_the_round_trip() {
        // The signed field is the whole reason `read_i32` exists. An immersion
        // rig below freezing must not come back as a positive number.
        let message = SubmitWorkerTelemetry {
            temperature_millicelsius: -18_000,
            ..plausible_telemetry()
        };

        assert_eq!(telemetry_round_trip(message).unwrap(), message);
    }

    #[test]
    fn a_fan_duty_cycle_above_one_hundred_is_refused_rather_than_clamped() {
        let message = SubmitWorkerTelemetry {
            fan_percent: 200,
            ..plausible_telemetry()
        };

        assert!(telemetry_round_trip(message).is_err());
    }

    #[test]
    fn a_zero_length_sample_window_is_refused() {
        // A consumer dividing power by this window would produce an infinity,
        // which is the failure `read_f32` refuses by a different route.
        let message = SubmitWorkerTelemetry {
            sample_millis: 0,
            ..plausible_telemetry()
        };

        assert!(telemetry_round_trip(message).is_err());
    }

    #[test]
    fn an_inverted_nonce_range_is_refused_on_decode() {
        let mut writer = Writer::new();
        SetNonceRange {
            channel_id: 1,
            nonce_min: 900,
            nonce_max: 100,
        }
        .encode(&mut writer);
        let bytes = writer.finish();

        assert!(SetNonceRange::decode(&mut Reader::new(&bytes)).is_err());
    }

    #[test]
    fn an_empty_nonce_range_is_refused_on_decode() {
        let mut writer = Writer::new();
        SetNonceRange {
            channel_id: 1,
            nonce_min: 100,
            nonce_max: 100,
        }
        .encode(&mut writer);
        let bytes = writer.finish();

        assert!(SetNonceRange::decode(&mut Reader::new(&bytes)).is_err());
    }

    #[test]
    fn a_nonce_range_is_half_open() {
        let range = SetNonceRange {
            channel_id: 1,
            nonce_min: 100,
            nonce_max: 200,
        };
        assert!(!range.contains(99));
        assert!(range.contains(100));
        assert!(range.contains(199));
        assert!(
            !range.contains(200),
            "an inclusive upper bound would overlap the next channel"
        );
    }

    #[test]
    fn adjacent_ranges_partition_without_overlapping() {
        // The property the whole scheme rests on: two channels can never be
        // handed the same nonce.
        let first = SetNonceRange {
            channel_id: 1,
            nonce_min: 0,
            nonce_max: 1_000,
        };
        let second = SetNonceRange {
            channel_id: 2,
            nonce_min: 1_000,
            nonce_max: 2_000,
        };
        for nonce in [0u64, 500, 999, 1_000, 1_500, 1_999] {
            assert!(
                !(first.contains(nonce) && second.contains(nonce)),
                "nonce {nonce} belongs to both channels"
            );
        }
    }
}
