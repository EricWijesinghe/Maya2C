//! One connection's protocol state machine.
//!
//! Deliberately free of I/O. [`Session::handle`] takes a decoded message and
//! returns what should happen next; the socket, the encryption, and the
//! validator queue all live elsewhere. That split is what makes the protocol
//! testable at all — every branch below can be exercised with a `Vec<Message>`
//! and no listener, and the 50-worker simulation drives these sessions directly
//! rather than through 50 sockets.
//!
//! ## What a connection is allowed to do
//!
//! - **Before setup**, only `SetupConnection`. A channel opened by a peer that
//!   has not negotiated a version is a channel whose protocol assumptions are
//!   unknown.
//! - **Only its own channels.** A message naming a channel this connection did
//!   not open is refused with `unknown-channel-id`, not looked up in the global
//!   registry. Without that check any connection could retarget, close, or
//!   submit against any other miner's channel by guessing a small integer.
//! - **No unsolicited work.** Jobs are pushed by the daemon, never requested.
//!
//! ## Expensive work leaves by a different door
//!
//! A share that survives the integer checks becomes a
//! [`SessionAction::Validate`], not a reply. Hashing on a connection task would
//! put 25 ms of Argon2id in front of every other message that connection has
//! queued, and at 50,000 connections would put it in front of the runtime.

use maya_stratum_v2::messages::mining::{
    CloseChannel, OpenMiningChannelError, OpenStandardMiningChannelSuccess, SetNewPrevHash,
    SetTarget, SubmitSharesError, SubmitSharesStandard, error_codes,
};
use maya_stratum_v2::{Message, Protocol};

use crate::error::Result;
use crate::model::{RejectReason, WorkerKey, now_millis, parse_user_identity};
use crate::state::{PoolEvent, PoolState};

/// Protocol version this build speaks.
pub const PROTOCOL_VERSION: u16 = 2;

/// Feature flags advertised. None yet; the field exists so a later build can
/// negotiate without a message change.
const FLAGS: u32 = 0;

/// Error code for a channel a connection does not own.
const UNKNOWN_CHANNEL: &str = error_codes::UNKNOWN_CHANNEL;

/// What the transport should do with a handled message.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionAction {
    /// Send this message back.
    Reply(Message),
    /// Verify this share off the connection task.
    Validate(ShareRequest),
    /// Close the connection; the string is for the log, not the peer.
    Close(String),
}

/// A share that passed every check that costs nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareRequest {
    /// Channel that submitted.
    pub channel_id: u32,
    /// Who to credit.
    pub key: WorkerKey,
    /// Sequence number, for the acknowledgement.
    pub sequence_number: u32,
    /// Job claimed.
    pub job_id: u32,
    /// Nonce claimed.
    pub nonce: u64,
    /// Header timestamp claimed.
    pub ntime: u64,
    /// The channel's share target when the share arrived.
    ///
    /// Captured here rather than read again at validation time. A retune
    /// between submission and verification must not change what the miner is
    /// credited: they did the work against the target they were holding.
    pub target: [u8; 32],
}

/// One connection.
#[derive(Debug, Default)]
pub struct Session {
    /// Whether `SetupConnection` has been accepted.
    setup: bool,
    /// Channels opened by this connection.
    channels: Vec<u32>,
}

impl Session {
    /// A fresh session.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Channels this connection owns.
    #[must_use]
    pub fn channels(&self) -> &[u32] {
        &self.channels
    }

    /// Whether the connection has completed setup.
    #[must_use]
    pub fn is_set_up(&self) -> bool {
        self.setup
    }

    /// Handles one decoded message.
    ///
    /// # Errors
    ///
    /// Propagates lock poisoning from [`PoolState`]. A *protocol* fault is
    /// never an error here — it is a reply or a close, because a peer that
    /// sends nonsense must not be able to take a code path meant for the pool
    /// having a problem.
    pub fn handle(&mut self, message: Message, state: &PoolState) -> Result<Vec<SessionAction>> {
        if !self.setup && !matches!(message, Message::SetupConnection(_)) {
            return Ok(vec![SessionAction::Close(
                "a message arrived before SetupConnection".to_string(),
            )]);
        }

        match message {
            Message::SetupConnection(setup) => Ok(self.on_setup(&setup)),
            Message::OpenStandardMiningChannel(open) => self.on_open(
                open.request_id,
                &open.user_identity,
                open.nominal_hash_rate,
                state,
            ),
            Message::UpdateChannel(update) => {
                self.on_update(update.channel_id, update.nominal_hash_rate, state)
            }
            Message::CloseChannel(close) => self.on_close(close.channel_id, state),
            Message::SubmitSharesStandard(submit) => self.on_submit(&submit, state),
            Message::SubmitWorkerTelemetry(telemetry) => self.on_telemetry(&telemetry, state),
            // Everything else is a message the *pool* sends. A miner sending one
            // is a client that has the protocol backwards, and answering it
            // would be answering a question nobody asked.
            other => Ok(vec![SessionAction::Close(format!(
                "a miner sent a pool-to-miner message: {:#04x}",
                other.msg_type()
            ))]),
        }
    }

    /// Negotiates the connection.
    fn on_setup(
        &mut self,
        setup: &maya_stratum_v2::messages::SetupConnection,
    ) -> Vec<SessionAction> {
        if setup.protocol != Protocol::Mining {
            return vec![SessionAction::Reply(Message::SetupConnectionError(
                maya_stratum_v2::messages::SetupConnectionError {
                    flags: setup.flags,
                    error_code: "unsupported-protocol".to_string(),
                },
            ))];
        }

        if PROTOCOL_VERSION < setup.min_version || PROTOCOL_VERSION > setup.max_version {
            return vec![SessionAction::Reply(Message::SetupConnectionError(
                maya_stratum_v2::messages::SetupConnectionError {
                    flags: setup.flags,
                    error_code: "protocol-version-mismatch".to_string(),
                },
            ))];
        }

        self.setup = true;
        vec![SessionAction::Reply(Message::SetupConnectionSuccess(
            maya_stratum_v2::messages::SetupConnectionSuccess {
                used_version: PROTOCOL_VERSION,
                flags: FLAGS,
            },
        ))]
    }

    /// Opens a channel and hands it its first job.
    fn on_open(
        &mut self,
        request_id: u32,
        identity: &str,
        reported_hashrate: f32,
        state: &PoolState,
    ) -> Result<Vec<SessionAction>> {
        let key = match parse_user_identity(identity) {
            Ok(key) => key,
            Err(error) => {
                // The miner's own mistake, and one they can fix — so it names
                // what was wrong rather than just refusing.
                return Ok(vec![SessionAction::Reply(Message::OpenMiningChannelError(
                    OpenMiningChannelError {
                        request_id,
                        error_code: format!("invalid-user-identity: {error}"),
                    },
                ))]);
            }
        };

        let now = now_millis();
        let jobs = state.jobs()?;
        let Some(job) = jobs.current() else {
            // No template yet: the node is unreachable or the pool has just
            // started. Refusing is honest; opening a channel with no work would
            // leave the rig idle with no way to know why.
            return Ok(vec![SessionAction::Reply(Message::OpenMiningChannelError(
                OpenMiningChannelError {
                    request_id,
                    error_code: "no-work-available".to_string(),
                },
            ))]);
        };

        let mut channels = state.channels()?;
        let Some(channel) = channels.open(key.clone(), reported_hashrate, &state.config, now)
        else {
            return Ok(vec![SessionAction::Reply(Message::OpenMiningChannelError(
                OpenMiningChannelError {
                    request_id,
                    error_code: "pool-capacity-reached".to_string(),
                },
            ))]);
        };

        let channel_id = channel.id;
        let range = channel.range;
        let target = channel.target();
        let bits = channel.vardiff.bits();
        self.channels.push(channel_id);

        let channel = channels.get_mut(channel_id).expect("just inserted");
        channel.push_job(job.id);

        drop(channels);

        state
            .telemetry()?
            .record_connection(&key, true, reported_hashrate, now);
        state.telemetry()?.record_target(&key, bits, now);

        Ok(vec![
            SessionAction::Reply(Message::OpenStandardMiningChannelSuccess(
                OpenStandardMiningChannelSuccess {
                    request_id,
                    channel_id,
                    target,
                    nonce_min: range.min,
                    nonce_max: range.max,
                },
            )),
            SessionAction::Reply(Message::SetNewPrevHash(SetNewPrevHash {
                channel_id,
                job_id: job.id,
                prev_hash: job.header.prev_hash,
                min_ntime: job.header.timestamp,
                target: job.network_target,
            })),
            SessionAction::Reply(Message::NewMiningJob(
                maya_stratum_v2::messages::NewMiningJob {
                    channel_id,
                    job_id: job.id,
                    state_root: job.header.state_root,
                    timestamp: job.header.timestamp,
                },
            )),
        ])
    }

    /// Records a revised hash-rate claim.
    fn on_update(
        &mut self,
        channel_id: u32,
        reported_hashrate: f32,
        state: &PoolState,
    ) -> Result<Vec<SessionAction>> {
        if !self.owns(channel_id) {
            return Ok(vec![SessionAction::Close(format!(
                "channel {channel_id} does not belong to this connection"
            ))]);
        }

        let (key, target) = {
            let channels = state.channels()?;
            let Some(channel) = channels.get(channel_id) else {
                return Ok(Vec::new());
            };
            (channel.key.clone(), channel.target())
        };

        state
            .telemetry()?
            .record_connection(&key, true, reported_hashrate, now_millis());

        // The claim does not move the target. Vardiff steers on measured share
        // arrival, and a controller that took the miner's word for its speed
        // would be a controller the miner sets.
        Ok(vec![SessionAction::Reply(Message::SetTarget(SetTarget {
            channel_id,
            maximum_target: target,
        }))])
    }

    /// Closes a channel.
    fn on_close(&mut self, channel_id: u32, state: &PoolState) -> Result<Vec<SessionAction>> {
        if !self.owns(channel_id) {
            return Ok(vec![SessionAction::Close(format!(
                "channel {channel_id} does not belong to this connection"
            ))]);
        }

        self.release(channel_id, state)?;
        Ok(Vec::new())
    }

    /// Screens a submission and hands the survivors to the validator.
    fn on_submit(
        &mut self,
        submit: &SubmitSharesStandard,
        state: &PoolState,
    ) -> Result<Vec<SessionAction>> {
        if !self.owns(submit.channel_id) {
            // Not a close: a stale submission for a channel this connection
            // just closed is ordinary. Naming the channel is enough.
            return Ok(vec![SessionAction::Reply(Message::SubmitSharesError(
                SubmitSharesError {
                    channel_id: submit.channel_id,
                    sequence_number: submit.sequence_number,
                    error_code: UNKNOWN_CHANNEL.to_string(),
                },
            ))]);
        }

        let (key, target, precheck) = {
            let mut channels = state.channels()?;
            let Some(channel) = channels.get_mut(submit.channel_id) else {
                return Ok(vec![SessionAction::Reply(Message::SubmitSharesError(
                    SubmitSharesError {
                        channel_id: submit.channel_id,
                        sequence_number: submit.sequence_number,
                        error_code: UNKNOWN_CHANNEL.to_string(),
                    },
                ))]);
            };

            let precheck = channel.precheck(submit.job_id, submit.nonce);
            if precheck.is_ok() {
                // Remembered now, not after verification. A nonce that fails
                // once fails every time, and re-hashing it is 25 ms spent to
                // learn nothing.
                channel.remember(submit.job_id, submit.nonce);
            }
            (channel.key.clone(), channel.target(), precheck)
        };

        if let Err(reason) = precheck {
            return Ok(vec![self.reject(submit, &key, reason, state)?]);
        }

        Ok(vec![SessionAction::Validate(ShareRequest {
            channel_id: submit.channel_id,
            key,
            sequence_number: submit.sequence_number,
            job_id: submit.job_id,
            nonce: submit.nonce,
            ntime: submit.ntime,
            target,
        })])
    }

    /// Records a rig's self-reported health.
    fn on_telemetry(
        &mut self,
        telemetry: &maya_stratum_v2::messages::mining::SubmitWorkerTelemetry,
        state: &PoolState,
    ) -> Result<Vec<SessionAction>> {
        if !self.owns(telemetry.channel_id) {
            return Ok(vec![SessionAction::Close(format!(
                "channel {} does not belong to this connection",
                telemetry.channel_id
            ))]);
        }

        let key = {
            let channels = state.channels()?;
            match channels.get(telemetry.channel_id) {
                Some(channel) => channel.key.clone(),
                None => return Ok(Vec::new()),
            }
        };

        state.telemetry()?.record_reported_health(
            &key,
            telemetry.power_milliwatts,
            telemetry.temperature_millicelsius,
            telemetry.fan_percent,
            now_millis(),
        );

        // No reply. Telemetry is a report, not a request, and acknowledging it
        // would double the message rate for information the pool did not ask
        // for and cannot check.
        Ok(Vec::new())
    }

    /// Closes every channel this connection opened.
    ///
    /// Called when the socket goes away. Without it a dropped connection leaks
    /// its nonce ranges and leaves its rigs showing as connected forever.
    ///
    /// # Errors
    ///
    /// Propagates lock poisoning.
    pub fn disconnect(&mut self, state: &PoolState) -> Result<()> {
        for channel_id in std::mem::take(&mut self.channels) {
            self.release(channel_id, state)?;
        }
        Ok(())
    }

    /// Drops one channel and marks its rig disconnected.
    fn release(&mut self, channel_id: u32, state: &PoolState) -> Result<()> {
        self.channels.retain(|held| *held != channel_id);

        let closed = state.channels()?.close(channel_id);
        if let Some(channel) = closed {
            state.telemetry()?.record_connection(
                &channel.key,
                false,
                channel.reported_hashrate,
                now_millis(),
            );
        }
        Ok(())
    }

    /// Builds a rejection, recording it on the way out.
    fn reject(
        &self,
        submit: &SubmitSharesStandard,
        key: &WorkerKey,
        reason: RejectReason,
        state: &PoolState,
    ) -> Result<SessionAction> {
        {
            let mut channels = state.channels()?;
            if let Some(channel) = channels.get_mut(submit.channel_id) {
                channel.rejected += 1;
                if reason == RejectReason::Stale {
                    channel.stale += 1;
                }
            }
        }

        state
            .telemetry()?
            .record_rejected(key, reason, now_millis());
        state.metrics.record_rejected(reason);
        state.publish(PoolEvent::Rejected {
            miner: hex::encode(key.miner),
            worker: key.worker.clone(),
            reason: reason.code(),
        });

        Ok(SessionAction::Reply(Message::SubmitSharesError(
            SubmitSharesError {
                channel_id: submit.channel_id,
                sequence_number: submit.sequence_number,
                error_code: reason.code().to_string(),
            },
        )))
    }

    /// Whether this connection opened `channel_id`.
    fn owns(&self, channel_id: u32) -> bool {
        self.channels.contains(&channel_id)
    }
}

/// The messages that tell one channel about a new job.
///
/// `SetNewPrevHash` first, then `NewMiningJob`: the order matters because a rig
/// that received the job first would, for the width of that gap, be mining the
/// new job against the previous block's parent hash.
#[must_use]
pub fn job_messages(channel_id: u32, job: &crate::job::Job) -> Vec<Message> {
    vec![
        Message::SetNewPrevHash(SetNewPrevHash {
            channel_id,
            job_id: job.id,
            prev_hash: job.header.prev_hash,
            min_ntime: job.header.timestamp,
            target: job.network_target,
        }),
        Message::NewMiningJob(maya_stratum_v2::messages::NewMiningJob {
            channel_id,
            job_id: job.id,
            state_root: job.header.state_root,
            timestamp: job.header.timestamp,
        }),
    ]
}

/// A `CloseChannel` naming why the pool dropped a channel.
#[must_use]
pub fn close_message(channel_id: u32, reason: &str) -> Message {
    Message::CloseChannel(CloseChannel {
        channel_id,
        reason_code: reason.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;

    use custom_l1_node::core::BlockHeader;
    use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
    use custom_l1_node::rpc::{HeaderInfo, MiningCandidate};
    use maya_stratum_v2::messages::{SetupConnection, UpdateChannel};

    use crate::config::PoolConfig;
    use crate::ledger::memory::MemoryLedger;
    use crate::metrics::PoolMetrics;

    const ADDRESS_HEX: &str = "11223344556677889900aabbccddeeff\
                               11223344556677889900aabbccddeeff";

    fn state_with_job() -> PoolState {
        let state = PoolState::new(
            PoolConfig {
                reward_per_block: 1_000,
                ..PoolConfig::default()
            },
            Arc::new(MemoryLedger::new()),
            Arc::new(PoolMetrics::new()),
            Arc::new(CacheRegistry::new(DagConfig::NEVER)),
        );

        let header = BlockHeader {
            prev_hash: [1u8; 32],
            state_root: [2u8; 32],
            timestamp: 1_700_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(24),
        };
        state
            .jobs_mut()
            .unwrap()
            .push(&MiningCandidate {
                height: 10,
                difficulty_target: hex::encode(header.difficulty_target),
                header_bytes: hex::encode(header.serialize()),
                header: HeaderInfo::from(&header),
            })
            .unwrap();

        state
    }

    fn setup_message() -> Message {
        Message::SetupConnection(SetupConnection {
            protocol: Protocol::Mining,
            min_version: 2,
            max_version: 2,
            flags: 0,
            endpoint_host: "pool".to_string(),
            endpoint_port: 3333,
            vendor: "test".to_string(),
            hardware_version: "1".to_string(),
            firmware: "1".to_string(),
            device_id: "rig".to_string(),
        })
    }

    /// A session that has completed setup and opened one channel.
    fn opened(state: &PoolState) -> (Session, u32, u64) {
        let mut session = Session::new();
        session.handle(setup_message(), state).unwrap();

        let actions = session
            .handle(
                Message::OpenStandardMiningChannel(
                    maya_stratum_v2::messages::OpenStandardMiningChannel {
                        request_id: 1,
                        user_identity: format!("{ADDRESS_HEX}.rig-1"),
                        nominal_hash_rate: 1_000.0,
                        max_target: [0xFF; 32],
                    },
                ),
                state,
            )
            .unwrap();

        let channel_id = match &actions[0] {
            SessionAction::Reply(Message::OpenStandardMiningChannelSuccess(success)) => {
                success.channel_id
            }
            other => panic!("expected a channel to open, got {other:?}"),
        };
        let nonce_min = state.channels().unwrap().get(channel_id).unwrap().range.min;
        (session, channel_id, nonce_min)
    }

    #[test]
    fn a_message_before_setup_closes_the_connection() {
        // A channel opened by a peer that has not negotiated a version is a
        // channel whose protocol assumptions are unknown.
        let state = state_with_job();
        let mut session = Session::new();

        let actions = session
            .handle(
                Message::SubmitSharesStandard(SubmitSharesStandard {
                    channel_id: 0,
                    sequence_number: 0,
                    job_id: 0,
                    nonce: 0,
                    ntime: 0,
                }),
                &state,
            )
            .unwrap();

        assert!(matches!(actions[0], SessionAction::Close(_)));
    }

    #[test]
    fn a_version_outside_the_negotiated_range_is_refused() {
        let state = state_with_job();
        let mut session = Session::new();

        let actions = session
            .handle(
                Message::SetupConnection(SetupConnection {
                    min_version: 5,
                    max_version: 9,
                    ..match setup_message() {
                        Message::SetupConnection(setup) => setup,
                        _ => unreachable!(),
                    }
                }),
                &state,
            )
            .unwrap();

        assert!(matches!(
            actions[0],
            SessionAction::Reply(Message::SetupConnectionError(_))
        ));
        assert!(!session.is_set_up());
    }

    #[test]
    fn opening_a_channel_hands_out_work_and_a_nonce_range() {
        let state = state_with_job();
        let mut session = Session::new();
        session.handle(setup_message(), &state).unwrap();

        let actions = session
            .handle(
                Message::OpenStandardMiningChannel(
                    maya_stratum_v2::messages::OpenStandardMiningChannel {
                        request_id: 42,
                        user_identity: format!("{ADDRESS_HEX}.rig-1"),
                        nominal_hash_rate: 1_000.0,
                        max_target: [0xFF; 32],
                    },
                ),
                &state,
            )
            .unwrap();

        assert_eq!(actions.len(), 3, "success, prev hash, and a job");
        match (&actions[0], &actions[1], &actions[2]) {
            (
                SessionAction::Reply(Message::OpenStandardMiningChannelSuccess(success)),
                SessionAction::Reply(Message::SetNewPrevHash(_)),
                SessionAction::Reply(Message::NewMiningJob(_)),
            ) => {
                assert!(success.nonce_max > success.nonce_min);
                assert_eq!(success.request_id, 42);
            }
            other => panic!("unexpected opening sequence: {other:?}"),
        }
    }

    #[test]
    fn an_identity_without_an_address_is_refused_with_a_reason() {
        let state = state_with_job();
        let mut session = Session::new();
        session.handle(setup_message(), &state).unwrap();

        let actions = session
            .handle(
                Message::OpenStandardMiningChannel(
                    maya_stratum_v2::messages::OpenStandardMiningChannel {
                        request_id: 1,
                        user_identity: "just-a-rig-name".to_string(),
                        nominal_hash_rate: 1.0,
                        max_target: [0xFF; 32],
                    },
                ),
                &state,
            )
            .unwrap();

        match &actions[0] {
            SessionAction::Reply(Message::OpenMiningChannelError(error)) => {
                assert!(error.error_code.starts_with("invalid-user-identity"));
            }
            other => panic!("expected a named refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_channel_cannot_open_before_the_pool_has_work() {
        // Opening one anyway would leave the rig idle with no way to know why.
        let state = PoolState::new(
            PoolConfig {
                reward_per_block: 1_000,
                ..PoolConfig::default()
            },
            Arc::new(MemoryLedger::new()),
            Arc::new(PoolMetrics::new()),
            Arc::new(CacheRegistry::new(DagConfig::NEVER)),
        );
        let mut session = Session::new();
        session.handle(setup_message(), &state).unwrap();

        let actions = session
            .handle(
                Message::OpenStandardMiningChannel(
                    maya_stratum_v2::messages::OpenStandardMiningChannel {
                        request_id: 1,
                        user_identity: ADDRESS_HEX.to_string(),
                        nominal_hash_rate: 1.0,
                        max_target: [0xFF; 32],
                    },
                ),
                &state,
            )
            .unwrap();

        match &actions[0] {
            SessionAction::Reply(Message::OpenMiningChannelError(error)) => {
                assert_eq!(error.error_code, "no-work-available");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_share_that_passes_the_cheap_checks_is_handed_to_the_validator() {
        let state = state_with_job();
        let (mut session, channel_id, nonce_min) = opened(&state);
        let job_id = state.jobs().unwrap().current().unwrap().id;

        let actions = session
            .handle(
                Message::SubmitSharesStandard(SubmitSharesStandard {
                    channel_id,
                    sequence_number: 1,
                    job_id,
                    nonce: nonce_min + 7,
                    ntime: 1_700_000_000,
                }),
                &state,
            )
            .unwrap();

        match &actions[0] {
            SessionAction::Validate(request) => {
                assert_eq!(request.nonce, nonce_min + 7);
                assert_eq!(request.key.worker, "rig-1");
            }
            other => panic!("expected validation work, got {other:?}"),
        }
    }

    #[test]
    fn a_nonce_outside_the_range_never_reaches_the_validator() {
        // The cheapest rejection the pool has, and the reason nonce ranges are
        // disjoint in the first place.
        let state = state_with_job();
        let (mut session, channel_id, nonce_min) = opened(&state);
        let job_id = state.jobs().unwrap().current().unwrap().id;

        let actions = session
            .handle(
                Message::SubmitSharesStandard(SubmitSharesStandard {
                    channel_id,
                    sequence_number: 1,
                    job_id,
                    nonce: nonce_min.wrapping_sub(1),
                    ntime: 1_700_000_000,
                }),
                &state,
            )
            .unwrap();

        match &actions[0] {
            SessionAction::Reply(Message::SubmitSharesError(error)) => {
                assert_eq!(error.error_code, error_codes::NONCE_OUT_OF_RANGE);
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn a_resubmitted_nonce_is_refused_without_a_second_verification() {
        let state = state_with_job();
        let (mut session, channel_id, nonce_min) = opened(&state);
        let job_id = state.jobs().unwrap().current().unwrap().id;

        let submit = SubmitSharesStandard {
            channel_id,
            sequence_number: 1,
            job_id,
            nonce: nonce_min + 3,
            ntime: 1_700_000_000,
        };

        assert!(matches!(
            session
                .handle(Message::SubmitSharesStandard(submit), &state)
                .unwrap()[0],
            SessionAction::Validate(_)
        ));

        match &session
            .handle(Message::SubmitSharesStandard(submit), &state)
            .unwrap()[0]
        {
            SessionAction::Reply(Message::SubmitSharesError(error)) => {
                assert_eq!(error.error_code, error_codes::DUPLICATE_SHARE);
            }
            other => panic!("expected a duplicate rejection, got {other:?}"),
        }
    }

    #[test]
    fn a_connection_cannot_touch_a_channel_it_did_not_open() {
        // Without the ownership check, any connection could retarget, close, or
        // submit against another miner's channel by guessing a small integer.
        let state = state_with_job();
        let (_owner, channel_id, _nonce) = opened(&state);

        let mut intruder = Session::new();
        intruder.handle(setup_message(), &state).unwrap();

        let actions = intruder
            .handle(
                Message::CloseChannel(CloseChannel {
                    channel_id,
                    reason_code: "mine-now".to_string(),
                }),
                &state,
            )
            .unwrap();
        assert!(matches!(actions[0], SessionAction::Close(_)));

        // And the channel is still open and still the owner's.
        assert!(state.channels().unwrap().get(channel_id).is_some());
    }

    #[test]
    fn a_submission_for_someone_elses_channel_is_an_unknown_channel_error() {
        let state = state_with_job();
        let (_owner, channel_id, nonce_min) = opened(&state);

        let mut intruder = Session::new();
        intruder.handle(setup_message(), &state).unwrap();

        let actions = intruder
            .handle(
                Message::SubmitSharesStandard(SubmitSharesStandard {
                    channel_id,
                    sequence_number: 1,
                    job_id: 0,
                    nonce: nonce_min,
                    ntime: 1_700_000_000,
                }),
                &state,
            )
            .unwrap();

        match &actions[0] {
            SessionAction::Reply(Message::SubmitSharesError(error)) => {
                assert_eq!(error.error_code, error_codes::UNKNOWN_CHANNEL);
            }
            other => panic!("expected an unknown-channel error, got {other:?}"),
        }
    }

    #[test]
    fn a_hash_rate_claim_does_not_move_the_target() {
        // A controller that took the miner's word for its speed is a controller
        // the miner sets.
        let state = state_with_job();
        let (mut session, channel_id, _nonce) = opened(&state);
        let before = state.channels().unwrap().get(channel_id).unwrap().target();

        session
            .handle(
                Message::UpdateChannel(UpdateChannel {
                    channel_id,
                    nominal_hash_rate: 1e12,
                    maximum_target: [0xFF; 32],
                }),
                &state,
            )
            .unwrap();

        let after = state.channels().unwrap().get(channel_id).unwrap().target();
        assert_eq!(before, after);
    }

    #[test]
    fn telemetry_is_recorded_and_not_acknowledged() {
        let state = state_with_job();
        let (mut session, channel_id, _nonce) = opened(&state);

        let actions = session
            .handle(
                Message::SubmitWorkerTelemetry(
                    maya_stratum_v2::messages::mining::SubmitWorkerTelemetry {
                        channel_id,
                        power_milliwatts: 1_450_000,
                        temperature_millicelsius: 62_000,
                        fan_percent: 70,
                        sample_millis: 10_000,
                    },
                ),
                &state,
            )
            .unwrap();

        assert!(actions.is_empty(), "a report is not a request");

        let key = WorkerKey {
            miner: hex::decode(ADDRESS_HEX).unwrap().try_into().unwrap(),
            worker: "rig-1".to_string(),
        };
        let stats = state.telemetry().unwrap().stats_for(&key, 0).unwrap();
        assert_eq!(stats.reported_power_milliwatts, 1_450_000);
    }

    #[test]
    fn disconnecting_releases_every_channel_and_its_nonce_range() {
        let state = state_with_job();
        let (mut session, channel_id, _nonce) = opened(&state);

        session.disconnect(&state).unwrap();

        assert!(state.channels().unwrap().get(channel_id).is_none());
        assert!(state.channels().unwrap().is_empty());
        assert!(session.channels().is_empty());
    }

    #[test]
    fn a_pool_to_miner_message_from_a_miner_closes_the_connection() {
        let state = state_with_job();
        let (mut session, channel_id, _nonce) = opened(&state);

        let actions = session
            .handle(
                Message::SetTarget(SetTarget {
                    channel_id,
                    maximum_target: [0x00; 32],
                }),
                &state,
            )
            .unwrap();

        assert!(matches!(actions[0], SessionAction::Close(_)));
    }

    #[test]
    fn new_work_names_the_previous_hash_before_the_job() {
        // A rig that received the job first would spend that gap mining the new
        // job against the old parent.
        let state = state_with_job();
        let jobs = state.jobs().unwrap();
        let messages = job_messages(1, jobs.current().unwrap());

        assert!(matches!(messages[0], Message::SetNewPrevHash(_)));
        assert!(matches!(messages[1], Message::NewMiningJob(_)));
    }
}
