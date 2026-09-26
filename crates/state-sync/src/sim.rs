//! A joining node syncing from `peers` serving peers over the deterministic
//! network model, some of them malicious. Virtual time comes from the model
//! (latency plus each server's uplink serialisation); verification is real
//! work on real bytes, and its CPU time is measured separately.

#![allow(clippy::cast_precision_loss)]

use std::time::Instant;

use maya_sim::{Duration, Instant as SimInstant, LinkModel, NodeId, World};

use crate::{ChunkProof, Downloader, Event, Manifest, SyntheticState, chunk_hash};

/// Network and adversary parameters.
#[derive(Clone, Debug)]
pub struct Setup {
    /// Serving peers (node ids 1 ..= peers).
    pub peers: u16,
    /// Peers that corrupt one chunk in three.
    pub malicious: Vec<u16>,
    /// Requests in flight per peer.
    pub per_peer: usize,
    /// Each server's uplink, bits per second.
    pub uplink_bps: u64,
    /// Seed.
    pub seed: u64,
}

/// What a sync run measured.
#[derive(Clone, Debug)]
pub struct Report {
    /// Virtual time from first request to last verified chunk.
    pub virtual_secs: f64,
    /// Bytes downloaded, good and bad.
    pub bytes: u64,
    /// Bytes in chunks that failed verification.
    pub wasted: u64,
    /// Peers banned.
    pub banned: Vec<u16>,
    /// Sum of all balances imported (proof every record was read).
    pub balance_sum: u128,
    /// Real CPU seconds spent building the manifest (the serving side).
    pub manifest_cpu_secs: f64,
    /// Real CPU seconds the joiner spent verifying chunks.
    pub verify_cpu_secs: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Req(u32),
    Send(u32),
    Resp(u32, Vec<u8>, ChunkProof),
}

/// Runs the sync.
pub fn simulate(state: SyntheticState, setup: &Setup) -> Report {
    let t = Instant::now();
    let manifest = Manifest::new((0..state.chunks()).map(|i| chunk_hash(&state.chunk(i))).collect());
    let manifest_cpu_secs = t.elapsed().as_secs_f64();
    let mut world: World<Msg> = World::new(setup.seed);
    world.net_mut().set_link(LinkModel { loss_ppm: 0, reorder_ppm: 0, ..LinkModel::wide_area() });
    let mut dl = Downloader::new(manifest.root(), manifest.chunks(), 1..=setup.peers, setup.per_peer);
    let mut busy_until = vec![0u64; usize::from(setup.peers) + 1];
    for r in dl.poll() {
        world.send(NodeId(0), NodeId(r.peer), Msg::Req(r.chunk));
    }
    let mut balance_sum = 0u128;
    let mut verify = std::time::Duration::ZERO;
    let mut finished_at = 0u64;
    world.run_until(SimInstant::START + Duration::from_secs(86_400), u64::MAX, |w, ev| {
        let now = ev.at.as_nanos();
        match ev.payload {
            Msg::Req(c) => {
                let bytes = state.chunk(c).len() as u64;
                let p = usize::from(ev.to.0);
                let depart = busy_until[p].max(now) + bytes * 8 * 1_000_000_000 / setup.uplink_bps;
                busy_until[p] = depart;
                w.timer(ev.to, Duration::from_nanos(depart - now), Msg::Send(c));
            }
            Msg::Send(c) => {
                let mut bytes = state.chunk(c);
                if setup.malicious.contains(&ev.to.0) && c % 3 == 0 && !bytes.is_empty() {
                    bytes[40] ^= 1; // one nonce bit
                }
                w.send(ev.to, NodeId(0), Msg::Resp(c, bytes, manifest.prove(c)));
            }
            Msg::Resp(c, bytes, proof) => {
                let t = Instant::now();
                let events = dl.receive(ev.from.0, c, &bytes, &proof);
                verify += t.elapsed();
                for e in events {
                    match e {
                        Event::Verified(_) => balance_sum += SyntheticState::balance_sum(&bytes),
                        Event::Complete => finished_at = now,
                        Event::Banned(_) => {}
                    }
                }
                for r in dl.poll() {
                    w.send(NodeId(0), NodeId(r.peer), Msg::Req(r.chunk));
                }
            }
        }
    });
    Report {
        virtual_secs: finished_at as f64 / 1e9,
        bytes: dl.bytes,
        wasted: dl.wasted,
        banned: dl.banned().iter().copied().collect(),
        balance_sum,
        manifest_cpu_secs,
        verify_cpu_secs: verify.as_secs_f64(),
    }
}
