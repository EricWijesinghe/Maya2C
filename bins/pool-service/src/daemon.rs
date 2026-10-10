//! The daemon: the Stratum listener, and the loops around it.
//!
//! ## Transport
//!
//! The pool reuses `src/network/pq` — Noise-style XX over X25519 with an
//! ML-KEM-768 exchange layered inside, `ChaCha20Poly1305`, 64 KiB frames.
//! `docs/stratum-v2.md` §6 gives the reasoning: SV2 specifies
//! `Noise_NX_secp256k1_…`, and adopting it would introduce secp256k1 — the one
//! classical primitive this codebase deliberately avoids — to buy interop that
//! is unreachable anyway.
//!
//! `pq::handshake::respond` is generic over *futures* `AsyncRead`/`AsyncWrite`
//! and a Tokio `TcpStream` implements Tokio's, which is the entire reason
//! `tokio-util`'s compat layer is a dependency.
//!
//! ## One task per connection, and no `select!` over the reader
//!
//! Job pushes arrive on their own task and leave through an outbound queue, so
//! the read loop never sits in a `select!` alongside a partially consumed
//! frame. A cancelled frame read would discard bytes already taken off the
//! socket and desynchronise the stream — a failure that appears as "this one
//! miner sends garbage" long after the cause.
//!
//! ## Verification happens inline, per connection
//!
//! The read loop awaits the validator rather than spawning. That serialises one
//! connection's shares, which is the correct backpressure: vardiff is holding
//! each channel near one share a minute, so a rig waiting on its own share is
//! waiting on work it just asked for. Spawning per share would let one
//! connection queue thousands of verifications and starve every other.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::{AsyncReadExt, AsyncWriteExt};
use maya_stratum_v2::{Frame, FrameHeader, MAX_PAYLOAD_LEN, Message};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio_util::compat::TokioAsyncReadCompatExt;

use custom_l1_node::network::pq::handshake;
use custom_l1_node::network::pq::stream::PqStream;

use crate::error::{PoolError, Result};
use crate::job::Job;
use crate::model::{ShareOutcome, now_millis};
use crate::node::NodeClient;
use crate::payout::PayoutEngine;
use crate::session::{Session, SessionAction, close_message, job_messages};
use crate::share;
use crate::state::{PoolEvent, PoolState};
use crate::validator::{FoundBlockShare, Validator};

/// Messages queued for one connection before it is considered wedged.
///
/// A miner that cannot absorb 64 messages is a miner whose socket has stopped
/// draining, and holding more for it would be holding memory on the pool's
/// behalf rather than theirs.
const OUTBOUND_CAPACITY: usize = 64;

/// Accepted shares acknowledged in one `SubmitSharesSuccess`.
///
/// SV2 acknowledges in bulk on purpose: at 50,000 connections a reply per
/// submission doubles the pool's write syscalls to carry no information the
/// miner did not already have. Eight is short enough that a rig sees progress
/// within a few minutes at the pool's target share interval.
const ACK_BATCH: u32 = 8;

/// How often the pool asks the node for fresh work.
///
/// Well under the 15-second block target: `docs/stratum-v2.md` §4 requires
/// sub-second job push, and half a second of polling plus the push itself
/// keeps the stale window in the hundreds of milliseconds.
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often the payout engine runs.
///
/// Payouts are not urgent — they are already waiting on tens of confirmations —
/// and each pass makes several RPC calls.
const SETTLE_INTERVAL: Duration = Duration::from_secs(30);

/// How often gauges are resampled and housekeeping runs.
///
/// Matches the node's own five-second cadence, comfortably under a fifteen
/// second scrape.
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(5);

/// Share records kept behind the current PPLNS window before pruning.
///
/// Four windows. Deep enough that a burst of found blocks cannot reach past the
/// pruned edge, shallow enough that the ledger does not grow without bound.
const PRUNE_WINDOW_MULTIPLE: u64 = 4;

/// A running pool.
#[derive(Debug)]
pub struct PoolServer {
    /// Address actually bound, which matters when the caller asked for port 0.
    pub address: SocketAddr,
}

/// Binds the Stratum listener and serves it on a background task.
///
/// # Errors
///
/// Returns [`PoolError::Server`] if the address cannot be bound.
pub async fn serve_stratum(
    address: SocketAddr,
    state: Arc<PoolState>,
    validator: Validator,
    jobs: broadcast::Sender<Arc<Job>>,
) -> Result<PoolServer> {
    let listener = TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;

    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                // A failed accept is not a reason to stop listening; the next
                // miner should still find a socket. Same reasoning as the
                // node's metrics exporter.
                Err(error) => {
                    eprintln!("pool: accept failed: {error}");
                    continue;
                }
            };

            let state = Arc::clone(&state);
            let validator = validator.clone();
            let jobs = jobs.subscribe();

            tokio::spawn(async move {
                if let Err(error) = serve_connection(stream, state, validator, jobs).await {
                    // Connection-level failures are ordinary: miners disconnect,
                    // Ð¸ a malformed frame is a client bug, not a pool one.
                    eprintln!("pool: connection {peer} ended: {error}");
                }
            });
        }
    });

    Ok(PoolServer { address: bound })
}

/// Drives one miner connection from handshake to close.
async fn serve_connection(
    stream: TcpStream,
    state: Arc<PoolState>,
    validator: Validator,
    mut jobs: broadcast::Receiver<Arc<Job>>,
) -> Result<()> {
    stream.set_nodelay(true)?;

    // The compat layer exists for exactly this line: the handshake is written
    // against futures' traits and a Tokio socket implements Tokio's.
    let mut compat = stream.compat();
    let keys = handshake::respond(&mut compat)
        .await
        .map_err(|e| PoolError::Server(format!("post-quantum handshake failed: {e}")))?;

    // `is_initiator: false` — the pool responds, so it opens with the key the
    // dialer seals with. Getting this backwards yields a stream that decrypts
    // to noise rather than one that fails to connect.
    let secure = PqStream::new(compat, keys, false);
    let (mut reader, writer) = secure.split();

    let (outbound, mut queued) = mpsc::channel::<Message>(OUTBOUND_CAPACITY);
    let mut writer = writer;
    let write_task = tokio::spawn(async move {
        while let Some(message) = queued.recv().await {
            let Ok(frame) = message.to_frame() else {
                // A message the pool itself could not encode is a pool bug, and
                // dropping the connection over it would hide that.
                eprintln!("pool: refusing to send an unencodable message");
                continue;
            };
            if writer.write_all(&frame.encode()).await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
    });

    // Job pushes travel by their own task, so the read loop never has to sit in
    // a `select!` beside a half-read frame.
    let channels: Arc<std::sync::Mutex<Vec<u32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let push_channels = Arc::clone(&channels);
    let push_outbound = outbound.clone();
    let push_task = tokio::spawn(async move {
        loop {
            let job = match jobs.recv().await {
                Ok(job) => job,
                // A connection that fell behind on job pushes skips to the
                // newest rather than replaying stale work.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };

            let held: Vec<u32> = match push_channels.lock() {
                Ok(held) => held.clone(),
                Err(_) => break,
            };
            for channel_id in held {
                for message in job_messages(channel_id, &job) {
                    if push_outbound.send(message).await.is_err() {
                        return;
                    }
                }
            }
        }
    });

    let mut session = Session::new();
    let mut acks = AckBook::default();

    let result = loop {
        let frame = match read_frame(&mut reader).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };

        let message = match Message::from_frame(&frame) {
            Ok(message) => message,
            // A frame the pool cannot parse ends the connection. It is not an
            // error worth retrying: a client that sent one will send another.
            Err(error) => break Err(PoolError::Protocol(error)),
        };

        let actions = session.handle(message, &state)?;
        if let Ok(mut held) = channels.lock() {
            *held = session.channels().to_vec();
        }

        let mut closing = None;
        for action in actions {
            match action {
                SessionAction::Reply(message) => {
                    if outbound.send(message).await.is_err() {
                        closing = Some("the connection stopped draining".to_string());
                    }
                }
                SessionAction::Close(reason) => closing = Some(reason),
                SessionAction::Validate(request) => {
                    let channel_id = request.channel_id;
                    let sequence_number = request.sequence_number;
                    let verdict = validator.submit(request).await?;

                    match verdict.outcome {
                        ShareOutcome::Accepted { weight, .. } => {
                            if let Some(ack) = acks.record(channel_id, sequence_number, weight)
                                && outbound.send(ack).await.is_err()
                            {
                                closing = Some("the connection stopped draining".to_string());
                            }
                        }
                        ShareOutcome::Rejected(reason) => {
                            // Flushed first: an acknowledgement covering shares
                            // *before* the rejected one must not arrive after
                            // it, or the miner cannot tell which failed.
                            if let Some(ack) = acks.flush(channel_id)
                                && outbound.send(ack).await.is_err()
                            {
                                closing = Some("the connection stopped draining".to_string());
                            }
                            let error = Message::SubmitSharesError(
                                maya_stratum_v2::messages::SubmitSharesError {
                                    channel_id,
                                    sequence_number,
                                    error_code: reason.code().to_string(),
                                },
                            );
                            if outbound.send(error).await.is_err() {
                                closing = Some("the connection stopped draining".to_string());
                            }
                        }
                    }

                    if let Some(target) = verdict.retune {
                        let retune = Message::SetTarget(maya_stratum_v2::messages::SetTarget {
                            channel_id,
                            maximum_target: target,
                        });
                        if outbound.send(retune).await.is_err() {
                            closing = Some("the connection stopped draining".to_string());
                        }
                    }
                }
            }
        }

        if let Some(reason) = closing {
            for channel_id in session.channels() {
                let _ = outbound
                    .send(close_message(*channel_id, "pool-closing"))
                    .await;
            }
            break Err(PoolError::Server(reason));
        }
    };

    // Whatever ended the connection, its channels have to go back: a leaked
    // channel holds a nonce range forever and leaves its rig showing as
    // connected on the dashboard.
    session.disconnect(&state)?;
    drop(outbound);
    push_task.abort();
    let _ = write_task.await;
    result
}

/// Reads one SV2 frame, or `None` at a clean end of stream.
async fn read_frame<R: futures::AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Frame>> {
    let mut header_bytes = [0u8; maya_stratum_v2::frame::FRAME_HEADER_LEN];
    match reader.read_exact(&mut header_bytes).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => {
            return Err(PoolError::Server(format!(
                "reading a frame header: {error}"
            )));
        }
    }

    let header = FrameHeader::decode(&header_bytes)?;
    // Checked before a byte is reserved. At 50,000 connections a length field is
    // an allocation primitive, which is why `MAX_PAYLOAD_LEN` is set from the
    // transport's ceiling rather than from the U24 field's 16 MiB.
    if header.msg_length as usize > MAX_PAYLOAD_LEN {
        return Err(PoolError::Protocol(
            maya_stratum_v2::Sv2Error::PayloadTooLarge {
                len: header.msg_length as usize,
                max: MAX_PAYLOAD_LEN,
            },
        ));
    }

    let mut payload = vec![0u8; header.msg_length as usize];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|error| PoolError::Server(format!("reading a frame payload: {error}")))?;

    let mut whole = header_bytes.to_vec();
    whole.extend_from_slice(&payload);
    Ok(Some(Frame::decode(&whole)?))
}

/// Pending acknowledgements, per channel.
///
/// SV2 acknowledges shares in bulk; this is the accumulator that makes that
/// true rather than aspirational.
#[derive(Debug, Default)]
struct AckBook {
    /// One entry per channel with unacknowledged shares.
    pending: std::collections::HashMap<u32, PendingAck>,
}

/// Shares accepted on one channel since its last acknowledgement.
#[derive(Debug, Default)]
struct PendingAck {
    /// Highest sequence number covered.
    last_sequence: u32,
    /// Shares accepted.
    count: u32,
    /// Weight credited.
    weight: u64,
}

impl AckBook {
    /// Records an accepted share, returning an acknowledgement when one is due.
    fn record(&mut self, channel_id: u32, sequence: u32, weight: u64) -> Option<Message> {
        let entry = self.pending.entry(channel_id).or_default();
        entry.last_sequence = sequence;
        entry.count += 1;
        entry.weight = entry.weight.saturating_add(weight);

        if entry.count >= ACK_BATCH {
            return self.flush(channel_id);
        }
        None
    }

    /// Emits an acknowledgement for whatever is pending on a channel.
    fn flush(&mut self, channel_id: u32) -> Option<Message> {
        let entry = self.pending.remove(&channel_id)?;
        if entry.count == 0 {
            return None;
        }

        Some(Message::SubmitSharesSuccess(
            maya_stratum_v2::messages::SubmitSharesSuccess {
                channel_id,
                last_sequence_number: entry.last_sequence,
                new_submits_accepted_count: entry.count,
                new_shares_sum: entry.weight,
            },
        ))
    }
}

/// Polls the node for work and pushes it to every channel.
///
/// # Errors
///
/// Never returns while the process is alive; failures are logged and retried,
/// because an unreachable node is a condition to survive rather than to exit
/// on.
pub async fn job_loop(
    state: Arc<PoolState>,
    node: Arc<NodeClient>,
    jobs: broadcast::Sender<Arc<Job>>,
) {
    let mut ticker = tokio::time::interval(JOB_POLL_INTERVAL);

    loop {
        ticker.tick().await;

        let candidate = match node.mining_candidate().await {
            Ok(candidate) => candidate,
            Err(error) => {
                eprintln!("pool: no work from the node: {error}");
                continue;
            }
        };

        let started = std::time::Instant::now();
        let pushed = {
            let mut registry = match state.jobs_mut() {
                Ok(registry) => registry,
                Err(error) => {
                    eprintln!("pool: {error}");
                    continue;
                }
            };

            // `None` means the node handed back the same work with a newer
            // timestamp, which it does on every call. The registry is what
            // decides that, so this loop cannot get it wrong by omission —
            // see `JobRegistry::push`.
            match registry.push(&candidate) {
                Ok(Some(id)) => registry.get(id).cloned(),
                Ok(None) => None,
                Err(error) => {
                    eprintln!("pool: {error}");
                    continue;
                }
            }
        };

        let Some(job) = pushed else {
            continue;
        };

        if let Ok(mut channels) = state.channels() {
            for channel in channels.iter_mut() {
                channel.push_job(job.id);
            }
        }

        let job = Arc::new(job);
        let _ = jobs.send(Arc::clone(&job));
        state
            .metrics
            .observe_job_push(started.elapsed().as_secs_f64());
        state.publish(PoolEvent::Job {
            id: job.id,
            height: job.height,
        });
    }
}

/// Submits solved blocks and credits their PPLNS windows.
pub async fn block_loop(
    state: Arc<PoolState>,
    node: Arc<NodeClient>,
    engine: Arc<PayoutEngine>,
    mut found: mpsc::Receiver<FoundBlockShare>,
) {
    while let Some(share) = found.recv().await {
        let job = {
            let Ok(jobs) = state.jobs() else { continue };
            jobs.get(share.job_id).cloned()
        };
        let Some(job) = job else {
            eprintln!("pool: a solved block's job was retired before it could be submitted");
            continue;
        };

        let block = share::block_for(&job, share.nonce, share.ntime);
        let id = hex::encode(block.header.id());

        if let Err(error) = node.submit_block(&block).await {
            // The share is already credited, which is right: the miner did the
            // work. What is lost is the block, and that must be loud.
            eprintln!("pool: the chain refused a block the pool verified: {error}");
            continue;
        }

        state.metrics.record_block_found();
        state.publish(PoolEvent::Block {
            id: id.clone(),
            height: job.height,
            finder: hex::encode(share.key.miner),
        });

        if let Err(error) = engine.credit_block(
            &id,
            job.height,
            share.sequence,
            share.key.miner,
            &job.network_target,
        ) {
            // The block is on the chain and its credits are not in the ledger.
            // Nothing here can fix that automatically, so it is reported in the
            // strongest terms the daemon has.
            eprintln!("pool: BLOCK {id} WAS SUBMITTED BUT NOT CREDITED: {error}");
        }
    }
}

/// Runs the payout engine on a timer.
pub async fn settle_loop(state: Arc<PoolState>, engine: Arc<PayoutEngine>) {
    let mut ticker = tokio::time::interval(SETTLE_INTERVAL);

    loop {
        ticker.tick().await;

        match engine.settle().await {
            Ok(report) => {
                state.metrics.record_block_orphaned(report.orphaned as u64);
                state.metrics.record_payouts(
                    report.broadcast as u64,
                    report.confirmed as u64,
                    report.value_broadcast,
                );
            }
            Err(PoolError::Treasury(reason)) => {
                // Never retried on its own: every case is a policy or custody
                // condition that no retry clears.
                state.metrics.record_payout_refused();
                eprintln!("pool: payouts are stopped — {reason}");
            }
            Err(error) => eprintln!("pool: settlement pass failed: {error}"),
        }
    }
}

/// Resamples gauges, eases idle channels, and prunes.
pub async fn maintenance_loop(state: Arc<PoolState>, node: Arc<NodeClient>, treasury: [u8; 32]) {
    let mut ticker = tokio::time::interval(MAINTENANCE_INTERVAL);

    loop {
        ticker.tick().await;
        let now = now_millis();

        if let Err(error) = state.sample_gauges(now) {
            eprintln!("pool: {error}");
        }

        // The treasury balance is the gauge to page on: this chain mints no
        // block reward, so an empty account means every payout stops.
        match node.account_info(&treasury).await {
            Ok(info) => state.metrics.set_treasury_balance(info.balance),
            Err(error) => eprintln!("pool: could not read the treasury balance: {error}"),
        }

        if let Ok(mut channels) = state.channels() {
            for channel in channels.iter_mut() {
                channel.vardiff.on_idle_check(now);
            }
        }

        if let Ok(mut telemetry) = state.telemetry() {
            telemetry.evict_stale(now);
        }

        prune(&state);
    }
}

/// Drops share records far behind the current PPLNS window.
fn prune(state: &PoolState) {
    let Ok(jobs) = state.jobs() else { return };
    let Some(job) = jobs.current() else { return };
    let network_target = job.network_target;
    drop(jobs);

    let Ok(window) = crate::pplns::window_size(&network_target, state.config.pplns_factor) else {
        return;
    };
    let Ok(Some(tip)) = state.ledger.tip_sequence() else {
        return;
    };

    // Walk back four windows' worth and prune below that. Deriving the cut from
    // the window itself rather than from a share count is what keeps the rule
    // correct when vardiff has moved: the ledger keeps a fixed amount of
    // *work*, not a fixed number of records.
    let Some(deep) = window.mul_div_u64(PRUNE_WINDOW_MULTIPLE, 1) else {
        // Four windows overflowed 256 bits, which means the window itself is
        // near the top of the range. Nothing to prune, and nothing to guess at.
        return;
    };
    let Ok(slice) = state.ledger.window(tip, deep) else {
        return;
    };
    if !slice.full {
        // The ledger does not yet hold four windows, so there is nothing behind
        // them to drop.
        return;
    }

    let Some(oldest) = slice.shares.last().map(|share| share.sequence) else {
        return;
    };
    if let Err(error) = state.ledger.prune_shares_below(oldest) {
        eprintln!("pool: pruning failed: {error}");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn acknowledgements_are_batched_rather_than_per_share() {
        // A reply per submission doubles the pool's write syscalls to carry no
        // information the miner did not already have.
        let mut acks = AckBook::default();

        for sequence in 0..ACK_BATCH - 1 {
            assert!(acks.record(1, sequence, 1_024).is_none());
        }

        match acks.record(1, ACK_BATCH - 1, 1_024) {
            Some(Message::SubmitSharesSuccess(success)) => {
                assert_eq!(success.new_submits_accepted_count, ACK_BATCH);
                assert_eq!(success.last_sequence_number, ACK_BATCH - 1);
                assert_eq!(success.new_shares_sum, u64::from(ACK_BATCH) * 1_024);
            }
            other => panic!("expected a batched acknowledgement, got {other:?}"),
        }
    }

    #[test]
    fn a_flush_covers_only_what_is_pending() {
        let mut acks = AckBook::default();
        acks.record(1, 0, 10);
        acks.record(1, 1, 10);

        match acks.flush(1) {
            Some(Message::SubmitSharesSuccess(success)) => {
                assert_eq!(success.new_submits_accepted_count, 2);
                assert_eq!(success.new_shares_sum, 20);
            }
            other => panic!("expected an acknowledgement, got {other:?}"),
        }

        // And nothing is left to acknowledge twice.
        assert!(acks.flush(1).is_none());
    }

    #[test]
    fn channels_are_acknowledged_independently() {
        // One rig's share rate must not decide when another's shares are
        // acknowledged.
        let mut acks = AckBook::default();
        for sequence in 0..ACK_BATCH {
            acks.record(1, sequence, 1);
        }
        acks.record(2, 0, 1);

        match acks.flush(2) {
            Some(Message::SubmitSharesSuccess(success)) => {
                assert_eq!(success.channel_id, 2);
                assert_eq!(success.new_submits_accepted_count, 1);
            }
            other => panic!("expected channel 2's own acknowledgement, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_truncated_frame_header_is_a_clean_end_of_stream() {
        // A miner that hangs up between messages is ordinary, and must not be
        // logged as a protocol violation.
        let empty: &[u8] = &[];
        let mut reader = futures::io::Cursor::new(empty);
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_frame_declaring_more_than_the_transport_can_carry_is_refused() {
        // Refused before a byte is reserved: at 50,000 connections a length
        // field is an allocation primitive.
        let mut header = [0u8; maya_stratum_v2::frame::FRAME_HEADER_LEN];
        header[0..2].copy_from_slice(&maya_stratum_v2::MAYA_EXTENSION_TYPE.to_le_bytes());
        header[2] = 0x1a;
        header[3..6].copy_from_slice(&[0xFF, 0xFF, 0xFF]);

        let mut reader = futures::io::Cursor::new(header.to_vec());
        assert!(read_frame(&mut reader).await.is_err());
    }

    #[tokio::test]
    async fn a_well_formed_frame_round_trips_off_the_wire() {
        let message =
            Message::SubmitSharesStandard(maya_stratum_v2::messages::SubmitSharesStandard {
                channel_id: 1,
                sequence_number: 2,
                job_id: 3,
                nonce: 4,
                ntime: 5,
            });
        let bytes = message.to_frame().unwrap().encode();

        let mut reader = futures::io::Cursor::new(bytes);
        let frame = read_frame(&mut reader).await.unwrap().unwrap();
        assert_eq!(Message::from_frame(&frame).unwrap(), message);
    }
}
