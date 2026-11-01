//! One simulated miner: a real encrypted Stratum connection, pre-solved shares,
//! and a reader that timestamps every acknowledgement.

use std::collections::BTreeMap;
use std::error::Error;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use custom_l1_node::crypto::dag::registry::CacheRegistry;
use custom_l1_node::crypto::pow::meets_target;
use custom_l1_node::network::pq::handshake;
use custom_l1_node::network::pq::stream::PqStream;
use futures::{AsyncReadExt, AsyncWriteExt};
use maya_pool_service::job::Job;
use maya_stratum_v2::frame::FRAME_HEADER_LEN;
use maya_stratum_v2::messages::common::SetupConnection;
use maya_stratum_v2::messages::mining::{
    OpenStandardMiningChannel, SubmitSharesStandard,
};
use maya_stratum_v2::{Frame, FrameHeader, Message, Protocol};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncReadCompatExt;

pub type BoxError = Box<dyn Error + Send + Sync>;

/// Run phases, broadcast by the orchestrator to every miner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Connecting and opening channels.
    Wait,
    /// Sending shares, starting at this instant.
    Load(Instant),
    /// Sending has stopped; acknowledgements are still arriving.
    Drain,
    /// A new job was pushed at this instant; miners time its arrival.
    JobPush(Instant),
    /// Everything is measured; miners tear down.
    Done,
}

/// One share a miner will send.
#[derive(Clone, Copy, Debug)]
pub struct PlannedShare {
    pub sequence: u32,
    pub nonce: u64,
    pub valid: bool,
}

/// What a miner measured.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub accepted: u64,
    pub rejected: u64,
    /// Valid shares the pool refused. Should be zero.
    pub valid_rejected: u64,
    /// Immediate per-share refusal latency, microseconds.
    pub rejected_latency_us: Vec<u32>,
    /// Latency until the batched acknowledgement covering an accepted share.
    pub accepted_latency_us: Vec<u32>,
    /// When the new job's `NewMiningJob` arrived, if it did.
    pub job_seen_at: Option<Instant>,
    /// Whether the connection ended before the run did.
    pub ended_early: bool,
}

/// Shares sent and not yet answered, by sequence number.
type Pending = Arc<Mutex<BTreeMap<u32, (Instant, bool)>>>;

/// Everything one miner needs to run its share.
pub struct MinerSpec {
    pub index: usize,
    pub addr: SocketAddr,
    pub job: Arc<Job>,
    pub dag: Arc<CacheRegistry>,
    pub events: usize,
    pub interval: Duration,
    pub invalid_ratio: f64,
}

/// Runs a miner from connect to teardown. Reports readiness through `ready`.
///
/// The connection is built inline rather than in a helper: the split halves of
/// the encrypted stream are not nameable from outside the transport module, and
/// naming them is not worth a public type alias.
pub async fn run_miner(
    spec: MinerSpec,
    mut phase: tokio::sync::watch::Receiver<Phase>,
    ready: Arc<std::sync::atomic::AtomicUsize>,
    tally: Arc<Mutex<Tally>>,
) -> Result<(), BoxError> {
    let tcp = TcpStream::connect(spec.addr).await?;
    tcp.set_nodelay(true)?;
    let mut compat = tcp.compat();
    let keys = handshake::initiate(&mut compat)
        .await
        .map_err(|e| format!("handshake: {e}"))?;
    let secure = PqStream::new(compat, keys, true);
    let (mut reader, mut writer) = secure.split();

    send(
        &mut writer,
        Message::SetupConnection(SetupConnection {
            protocol: Protocol::Mining,
            min_version: 2,
            max_version: 2,
            flags: 0,
            endpoint_host: "127.0.0.1".to_string(),
            endpoint_port: 0,
            vendor: "pool-load".to_string(),
            hardware_version: "simulated".to_string(),
            firmware: "simulated".to_string(),
            device_id: format!("rig-{}", spec.index),
        }),
    )
    .await?;
    await_reply(&mut reader, |m| matches!(m, Message::SetupConnectionSuccess(_))).await?;

    let identity = format!("{:064x}.rig-{}", spec.index as u64 + 1, spec.index);
    send(
        &mut writer,
        Message::OpenStandardMiningChannel(OpenStandardMiningChannel {
            request_id: 1,
            user_identity: identity,
            nominal_hash_rate: 1.0e6,
            max_target: [0xff; 32],
        }),
    )
    .await?;
    let Message::OpenStandardMiningChannelSuccess(open) =
        await_reply(&mut reader, |m| matches!(m, Message::OpenStandardMiningChannelSuccess(_)))
            .await?
    else {
        return Err("no channel".into());
    };

    let plan = plan_shares(&spec, open.target, open.nonce_min, open.nonce_max).await?;
    ready.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    let Phase::Load(start) = wait_for(&mut phase, |p| matches!(p, Phase::Load(_))).await? else {
        return Ok(());
    };

    let pending: Pending = Arc::new(Mutex::new(BTreeMap::new()));
    let reader_pending = Arc::clone(&pending);
    let reader_tally = Arc::clone(&tally);
    let reader_task = tokio::spawn(async move {
        read_loop(&mut reader, &reader_pending, &reader_tally).await;
    });

    let offset = spec.interval.mul_f64((spec.index % 997) as f64 / 997.0);
    for (i, share) in plan.iter().enumerate() {
        let due = start + offset + spec.interval.saturating_mul(i as u32);
        tokio::time::sleep_until(due.into()).await;
        pending
            .lock()
            .map_err(|_| "pending map poisoned")?
            .insert(share.sequence, (Instant::now(), share.valid));
        let message = Message::SubmitSharesStandard(SubmitSharesStandard {
            channel_id: open.channel_id,
            sequence_number: share.sequence,
            job_id: spec.job.id,
            nonce: share.nonce,
            ntime: spec.job.header.timestamp,
        });
        if send(&mut writer, message).await.is_err() {
            tally.lock().map_err(|_| "tally poisoned")?.ended_early = true;
            break;
        }
    }

    wait_for(&mut phase, |p| matches!(p, Phase::JobPush(_) | Phase::Done)).await?;
    wait_for(&mut phase, |p| matches!(p, Phase::Done)).await?;
    reader_task.abort();
    Ok(())
}

/// Writes one message as a frame.
async fn send<W: futures::AsyncWrite + Unpin>(writer: &mut W, message: Message) -> Result<(), BoxError> {
    writer.write_all(&message.to_frame()?.encode()).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads frames until one matches `want`. Anything else arriving first (a job,
/// a target) is skipped, as a real rig would skip what it did not ask for.
async fn await_reply<R, F>(reader: &mut R, want: F) -> Result<Message, BoxError>
where
    R: futures::AsyncRead + Unpin,
    F: Fn(&Message) -> bool,
{
    loop {
        let Some(message) = read_message(reader).await? else {
            return Err("connection closed during setup".into());
        };
        if want(&message) {
            return Ok(message);
        }
        if let Message::SetupConnectionError(e) = message {
            return Err(format!("setup refused: {}", e.error_code).into());
        }
        if let Message::OpenMiningChannelError(e) = message {
            return Err(format!("channel refused: {}", e.error_code).into());
        }
    }
}

/// Reads one frame, or `None` at a clean end of stream.
pub async fn read_message<R: futures::AsyncRead + Unpin>(reader: &mut R) -> Result<Option<Message>, BoxError> {
    let mut header = [0u8; FRAME_HEADER_LEN];
    match reader.read_exact(&mut header).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let decoded = FrameHeader::decode(&header).map_err(|e| format!("frame header: {e}"))?;
    let mut whole = header.to_vec();
    whole.resize(FRAME_HEADER_LEN + decoded.msg_length as usize, 0);
    reader.read_exact(&mut whole[FRAME_HEADER_LEN..]).await?;
    let frame = Frame::decode(&whole).map_err(|e| format!("frame: {e}"))?;
    Ok(Some(Message::from_frame(&frame).map_err(|e| format!("message: {e}"))?))
}

/// Timestamps every reply. Runs until the connection ends or the task is aborted.
async fn read_loop<R: futures::AsyncRead + Unpin>(
    reader: &mut R,
    pending: &Pending,
    tally: &Mutex<Tally>,
) {
    loop {
        let message = match read_message(reader).await {
            Ok(Some(message)) => message,
            Ok(None) | Err(_) => {
                if let Ok(mut t) = tally.lock() {
                    t.ended_early = true;
                }
                return;
            }
        };
        let now = Instant::now();
        let Ok(mut t) = tally.lock() else { return };
        match message {
            Message::SubmitSharesSuccess(ack) => {
                t.accepted += u64::from(ack.new_submits_accepted_count);
                let Ok(mut map) = pending.lock() else { return };
                let mut rest = map.split_off(&(ack.last_sequence_number.saturating_add(1)));
                std::mem::swap(&mut *map, &mut rest);
                for (sent, valid) in rest.into_values() {
                    if valid {
                        t.accepted_latency_us.push(micros(now.duration_since(sent)));
                    }
                }
            }
            Message::SubmitSharesError(error) => {
                t.rejected += 1;
                let Ok(mut map) = pending.lock() else { return };
                if let Some((sent, valid)) = map.remove(&error.sequence_number) {
                    if valid {
                        t.valid_rejected += 1;
                    }
                    t.rejected_latency_us.push(micros(now.duration_since(sent)));
                }
            }
            Message::NewMiningJob(job) if job.job_id != 0 && t.job_seen_at.is_none() => {
                t.job_seen_at = Some(now);
            }
            _ => {}
        }
    }
}

fn micros(duration: Duration) -> u32 {
    u32::try_from(duration.as_micros()).unwrap_or(u32::MAX)
}

/// Waits until `pred` holds for the current phase, and returns it.
async fn wait_for(
    phase: &mut tokio::sync::watch::Receiver<Phase>,
    pred: impl Fn(Phase) -> bool,
) -> Result<Phase, BoxError> {
    loop {
        let current = *phase.borrow_and_update();
        if pred(current) {
            return Ok(current);
        }
        phase.changed().await?;
    }
}

/// Solves the miner's shares before the timed phase starts.
///
/// Valid shares are real solutions to the channel target, found by hashing the
/// pool's own header with the chain's DAG rule. Invalid shares are nonces whose
/// digest misses the target. Both are cheap to produce here and cost the pool a
/// full hash each.
async fn plan_shares(
    spec: &MinerSpec,
    target: [u8; 32],
    nonce_min: u64,
    nonce_max: u64,
) -> Result<Vec<PlannedShare>, BoxError> {
    let job = Arc::clone(&spec.job);
    let dag = Arc::clone(&spec.dag);
    let events = spec.events;
    let invalid_ratio = spec.invalid_ratio;
    tokio::task::spawn_blocking(move || {
        let mut out = Vec::with_capacity(events);
        let mut nonce = nonce_min;
        let invalid_every = if invalid_ratio > 0.0 {
            (1.0 / invalid_ratio).round().max(1.0) as usize
        } else {
            usize::MAX
        };
        for i in 0..events {
            let valid = (i + 1) % invalid_every != 0;
            loop {
                if nonce > nonce_max {
                    return Err::<Vec<PlannedShare>, BoxError>(
                        "the channel's nonce range is exhausted".into(),
                    );
                }
                let digest = job
                    .header_with(nonce, job.header.timestamp)
                    .pow_hash_at(job.height, &dag)
                    .map_err(|e| format!("hash: {e}"))?;
                let hit = meets_target(&digest, &target);
                let wanted = hit == valid;
                nonce += 1;
                if wanted {
                    out.push(PlannedShare {
                        sequence: u32::try_from(i + 1).unwrap_or(u32::MAX),
                        nonce: nonce - 1,
                        valid,
                    });
                    break;
                }
            }
        }
        Ok(out)
    })
    .await?
}
