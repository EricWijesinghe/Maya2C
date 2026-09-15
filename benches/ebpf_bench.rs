//! Block relay benchmark: parsing, the per-datagram cost, and the kernel paths.
//!
//! Answers four questions, in this order:
//!
//! 1. **What does a relay datagram cost to parse?** The fixed 56-byte header
//!    (`RelayHeader::split`) against the same fields in a verified flatbuffer.
//!    Flatbuffers is here as the baseline, not the format: the kernel's
//!    verifier cannot walk its offset tables, and its encoding is not
//!    canonical. Both are validated to the same rules.
//! 2. **What does a datagram cost end to end in user space?** Header, key
//!    lookup, XChaCha20-Poly1305, reassembly. At 100 Gbps a line of 1,232-byte
//!    datagrams is about ten million a second, so this — not parsing — is the
//!    number that decides how many cores a relay needs.
//! 3. **Loopback UDP with the full receiver.** Any platform. Syscalls included.
//! 4. **Kernel paths** (Linux, `--features xdp`, root, a veth pair): one `recv`
//!    per datagram, `recvmmsg` in batches of 64, AF_XDP, and `XDP_DROP`. This is
//!    the throughput comparison. It prints what it measured and whether the
//!    traffic generator was the bottleneck; it asserts no ratio, because the
//!    ratio belongs to a NIC and a driver, not to this code.
//!
//! Run:
//! ```text
//! cargo bench --bench ebpf_bench
//!
//! # kernel paths
//! (cd ebpf-net/programs && cargo build --release)
//! sudo ./scripts/xdp_netns.sh up
//! sudo -E MAYA_XDP_BENCH=1 \
//!   MAYA_XDP_OBJECT=$PWD/ebpf-net/programs/target/bpfel-unknown-none/release/maya-relay-xdp \
//!   cargo bench --features xdp --bench ebpf_bench
//! sudo ./scripts/xdp_netns.sh down
//! ```

use std::hint::black_box;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, Throughput};
use maya_ebpf_net::common::header::{CHUNK_LEN, HEADER_LEN, RelayHeader};
use maya_ebpf_net::common::rate::RateLimit;
use maya_ebpf_net::common::verdict::judge;
use maya_ebpf_net::{BlockSealer, KeyBook, Limits, RelayKey, RelayReceiver};

/// How long the loopback measurement counts.
const LOOPBACK_WINDOW: Duration = Duration::from_secs(2);

/// Block size for the reassembly and loopback measurements.
const BLOCK_BYTES: usize = 1 << 20;

/// The relay chunk as a flatbuffers table, written out by hand: one table does
/// not justify a `flatc` in the build.
mod flat {
    use flatbuffers::{
        FlatBufferBuilder, Follow, ForwardsUOffset, InvalidFlatbuffer, Table, VOffsetT, Verifiable,
        Vector, Verifier,
    };
    use maya_ebpf_net::common::header::{RelayHeader, TAG_LEN};

    const KEY_ID: VOffsetT = 4;
    const BLOCK_ID: VOffsetT = 6;
    const CHUNK_INDEX: VOffsetT = 8;
    const CHUNK_COUNT: VOffsetT = 10;
    const BODY_LEN: VOffsetT = 12;
    const SEALED: VOffsetT = 14;

    pub struct Chunk<'a> {
        table: Table<'a>,
    }

    impl<'a> Follow<'a> for Chunk<'a> {
        type Inner = Self;

        unsafe fn follow(buf: &'a [u8], loc: usize) -> Self {
            // SAFETY: `follow` is only reached through `flatbuffers::root`, after
            // `run_verifier` below has checked a table at `loc`.
            Self {
                table: unsafe { Table::new(buf, loc) },
            }
        }
    }

    impl Verifiable for Chunk<'_> {
        fn run_verifier(v: &mut Verifier, pos: usize) -> Result<(), InvalidFlatbuffer> {
            v.visit_table(pos)?
                .visit_field::<u64>("key_id", KEY_ID, true)?
                .visit_field::<ForwardsUOffset<Vector<u8>>>("block_id", BLOCK_ID, true)?
                .visit_field::<u16>("chunk_index", CHUNK_INDEX, true)?
                .visit_field::<u16>("chunk_count", CHUNK_COUNT, true)?
                .visit_field::<u32>("body_len", BODY_LEN, true)?
                .visit_field::<ForwardsUOffset<Vector<u8>>>("sealed", SEALED, true)?
                .finish();
            Ok(())
        }
    }

    impl<'a> Chunk<'a> {
        fn scalar<T: Follow<'a, Inner = T> + 'a>(&self, slot: VOffsetT) -> Option<T> {
            // SAFETY: every slot read here was verified as exactly this type in
            // `run_verifier`, which `flatbuffers::root` ran before this value
            // could exist.
            unsafe { self.table.get::<T>(slot, None) }
        }

        fn bytes(&self, slot: VOffsetT) -> Option<&'a [u8]> {
            // SAFETY: as `scalar`; the slot was verified as a byte vector.
            unsafe { self.table.get::<ForwardsUOffset<Vector<'a, u8>>>(slot, None) }
                .map(|vector| vector.bytes())
        }
    }

    pub fn encode(builder: &mut FlatBufferBuilder<'_>, header: &RelayHeader, sealed: &[u8]) -> Vec<u8> {
        builder.reset();
        let block_id = builder.create_vector(header.block_id().as_slice());
        let sealed = builder.create_vector(sealed);
        let start = builder.start_table();
        builder.push_slot_always::<u64>(KEY_ID, header.key_id());
        builder.push_slot_always(BLOCK_ID, block_id);
        builder.push_slot_always::<u16>(CHUNK_INDEX, header.chunk_index());
        builder.push_slot_always::<u16>(CHUNK_COUNT, header.chunk_count());
        builder.push_slot_always::<u32>(BODY_LEN, header.body_len());
        builder.push_slot_always(SEALED, sealed);
        let table = builder.end_table(start);
        builder.finish(table, None);
        builder.finished_data().to_vec()
    }

    /// Verifies, reads every field, and applies the relay header's rules: the
    /// same work `RelayHeader::split` does, so the comparison is like for like.
    pub fn parse(bytes: &[u8]) -> Option<(RelayHeader, &[u8])> {
        let chunk = flatbuffers::root::<Chunk>(bytes).ok()?;
        let block_id: [u8; 32] = chunk.bytes(BLOCK_ID)?.try_into().ok()?;
        let header = RelayHeader::new(
            chunk.scalar::<u64>(KEY_ID)?,
            block_id,
            chunk.scalar::<u16>(CHUNK_INDEX)?,
            chunk.scalar::<u32>(BODY_LEN)?,
        )
        .ok()?;
        let sealed = chunk.bytes(SEALED)?;
        let consistent = chunk.scalar::<u16>(CHUNK_COUNT)? == header.chunk_count()
            && sealed.len() == header.plaintext_len() + TAG_LEN;
        consistent.then_some((header, sealed))
    }
}

struct Fixture {
    key: RelayKey,
    datagram: Vec<u8>,
    flat: Vec<u8>,
}

fn fixture() -> Fixture {
    let key = RelayKey::from_bytes(&[3; 32]);
    let body = vec![0x42u8; CHUNK_LEN * 2];
    let sealer = BlockSealer::new(&key, [7; 32], &body).expect("sealer");
    let mut datagram = Vec::new();
    sealer.seal(0, &mut datagram).expect("seal");
    let (header, sealed) = RelayHeader::split(&datagram).expect("valid datagram");
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(2048);
    let flat = flat::encode(&mut builder, &header, sealed);
    assert_eq!(flat::parse(&flat).map(|(h, _)| h), Some(header), "the baseline must parse");
    Fixture { key, datagram, flat }
}

fn parsing(c: &mut Criterion, fixture: &Fixture) {
    let mut group = c.benchmark_group("parse");
    group.bench_function("relay_header_56_bytes", |b| {
        b.iter(|| {
            RelayHeader::split(black_box(&fixture.datagram))
                .map(|(header, sealed)| (header.chunk_index(), sealed.len()))
        });
    });
    group.bench_function("flatbuffers_verified", |b| {
        b.iter(|| flat::parse(black_box(&fixture.flat)).map(|(header, sealed)| (header.chunk_index(), sealed.len())));
    });
    group.bench_function("kernel_verdict", |b| {
        let head = fixture.datagram.first_chunk::<HEADER_LEN>();
        let limit = RateLimit::DEFAULT;
        let bucket = limit.first(0);
        b.iter(|| judge(None, bucket, limit, 1, black_box(head), black_box(fixture.datagram.len())));
    });
    group.finish();
}

fn datagram_costs(c: &mut Criterion, fixture: &Fixture) {
    let mut group = c.benchmark_group("datagram");
    let (header, sealed) = RelayHeader::split(&fixture.datagram).expect("valid datagram");
    let plaintext = vec![0x42u8; header.plaintext_len()];
    group.throughput(Throughput::Bytes(CHUNK_LEN as u64));
    group.bench_function("seal_1160", |b| {
        let mut out = Vec::with_capacity(fixture.datagram.len());
        b.iter(|| fixture.key.seal(&header, black_box(&plaintext), &mut out));
    });
    group.bench_function("open_1160", |b| {
        let mut out = vec![0u8; header.plaintext_len()];
        b.iter(|| fixture.key.open(&header, black_box(sealed), &mut out));
    });
    // The cipher alone over the same 1,160 bytes, with a fixed nonce and a
    // cipher built once: what `open_1160` would cost with no per-chunk work
    // around the AEAD. The gap between the two is the relay's own overhead.
    group.bench_function("raw_xchacha20poly1305_encrypt_1160", |b| {
        use chacha20poly1305::aead::{AeadInPlace, KeyInit};
        let cipher = chacha20poly1305::XChaCha20Poly1305::new(&[7u8; 32].into());
        let nonce = chacha20poly1305::XNonce::from([9u8; 24]);
        let mut buffer = plaintext.clone();
        b.iter(|| {
            buffer.copy_from_slice(&plaintext);
            cipher.encrypt_in_place_detached(&nonce, &[], black_box(&mut buffer))
        });
    });
    group.finish();

    let body: Vec<u8> = (0..BLOCK_BYTES).map(|i| (i % 251) as u8).collect();
    let block_key = RelayKey::from_bytes(&[5; 32]);
    let datagrams = sealed_block(&block_key, &body);
    let mut group = c.benchmark_group("receiver");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(BLOCK_BYTES as u64));
    group.bench_function("ingest_1_mib_block", |b| {
        b.iter_batched(
            || receiver_for(&block_key),
            |mut receiver| {
                let now = Instant::now();
                let mut delivered = 0;
                for datagram in &datagrams {
                    if let Ok(Some(_)) = receiver.ingest(datagram, now) {
                        delivered += 1;
                    }
                }
                assert_eq!(delivered, 1);
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

fn sealed_block(key: &RelayKey, body: &[u8]) -> Vec<Vec<u8>> {
    let sealer = BlockSealer::new(key, [9; 32], body).expect("sealer");
    (0..sealer.chunk_count())
        .map(|index| {
            let mut out = Vec::new();
            sealer.seal(index, &mut out).expect("seal");
            out
        })
        .collect()
}

/// A receiver holding `key` as peer 0's, remembering no deliveries, so the same
/// block can be delivered on every iteration.
fn receiver_for(key: &RelayKey) -> RelayReceiver<u8> {
    let book = KeyBook::shared();
    book.write()
        .expect("fresh lock")
        .insert(0, RelayKey::from_bytes(&key_bytes(key)));
    RelayReceiver::new(
        book,
        Limits {
            remembered: 0,
            ..Limits::DEFAULT
        },
    )
}

/// The benchmark's keys are built from known bytes; recover them by id.
fn key_bytes(key: &RelayKey) -> [u8; 32] {
    [3u8, 5]
        .into_iter()
        .map(|byte| [byte; 32])
        .find(|bytes| RelayKey::from_bytes(bytes).id() == key.id())
        .expect("a benchmark key")
}

fn loopback_udp() {
    let receiver_socket = UdpSocket::bind("127.0.0.1:0").expect("bind");
    receiver_socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("timeout");
    let target = receiver_socket.local_addr().expect("address");
    let key = RelayKey::from_bytes(&[5; 32]);
    let body: Vec<u8> = (0..BLOCK_BYTES).map(|i| (i % 251) as u8).collect();
    let datagrams = Arc::new(sealed_block(&key, &body));
    let mut receiver = receiver_for(&key);

    let stop = Arc::new(AtomicBool::new(false));
    let sender = {
        let (stop, datagrams) = (Arc::clone(&stop), Arc::clone(&datagrams));
        std::thread::spawn(move || {
            let socket = UdpSocket::bind("127.0.0.1:0").expect("bind sender");
            while !stop.load(Ordering::Relaxed) {
                for datagram in datagrams.iter() {
                    let _ = socket.send_to(datagram, target);
                }
            }
        })
    };

    let mut buffer = vec![0u8; 65_536];
    let start = Instant::now();
    let mut received = 0u64;
    while start.elapsed() < LOOPBACK_WINDOW {
        if let Ok(len) = receiver_socket.recv(&mut buffer) {
            received += 1;
            let _ = receiver.ingest(&buffer[..len], Instant::now());
        }
    }
    let elapsed = start.elapsed();
    stop.store(true, Ordering::Relaxed);
    sender.join().expect("sender");

    let stats = receiver.stats();
    let per_second = received as f64 / elapsed.as_secs_f64();
    println!();
    println!("loopback UDP, full receiver, one core each side");
    println!(
        "  {per_second:>12.0} datagrams/s received  ({:.1} µs each, syscall included)",
        1e6 / per_second.max(1.0)
    );
    println!(
        "  {:>12} opened and stored, {} blocks delivered, {} forged, {} refused",
        stats.accepted, stats.delivered, stats.forged, stats.refused
    );
    println!("  A sender faster than the receiver loses datagrams in the socket buffer,");
    println!("  so blocks delivered undercounts; datagrams/s is the receiver's rate.");
}

#[cfg(all(target_os = "linux", feature = "xdp"))]
fn kernel_paths() {
    use maya_ebpf_net::linux::throughput::{Measurement, ThroughputConfig, run_all};
    use maya_ebpf_net::{AttachMode, ZeroCopy};

    fn var(name: &str, default: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| default.to_string())
    }

    println!();
    if std::env::var("MAYA_XDP_BENCH").as_deref() != Ok("1") {
        println!("kernel paths: skipped; set MAYA_XDP_BENCH=1 (see scripts/xdp_netns.sh)");
        return;
    }
    let Ok(object) = std::env::var("MAYA_XDP_OBJECT") else {
        println!("kernel paths: skipped; MAYA_XDP_OBJECT must name the compiled program");
        return;
    };
    let cores = std::thread::available_parallelism().map_or(2, usize::from);
    let parsed = (|| -> Result<ThroughputConfig, String> {
        Ok(ThroughputConfig {
            interface: var("MAYA_XDP_IFACE", "maya-xdp0"),
            namespace: var("MAYA_XDP_NETNS", "maya-xdp-peer"),
            local: var("MAYA_XDP_LOCAL", "10.201.0.1").parse().map_err(|e| format!("MAYA_XDP_LOCAL: {e}"))?,
            peer: var("MAYA_XDP_PEER", "10.201.0.2").parse().map_err(|e| format!("MAYA_XDP_PEER: {e}"))?,
            port: var("MAYA_XDP_PORT", "30334").parse().map_err(|e| format!("MAYA_XDP_PORT: {e}"))?,
            object: object.into(),
            duration: Duration::from_secs(
                var("MAYA_XDP_SECONDS", "5").parse().map_err(|e| format!("MAYA_XDP_SECONDS: {e}"))?,
            ),
            senders: var("MAYA_XDP_SENDERS", &(cores / 2).max(1).to_string())
                .parse()
                .map_err(|e| format!("MAYA_XDP_SENDERS: {e}"))?,
            attach: if var("MAYA_XDP_MODE", "generic") == "driver" {
                AttachMode::Driver
            } else {
                AttachMode::Generic
            },
            zero_copy: if var("MAYA_XDP_ZEROCOPY", "off") == "on" {
                ZeroCopy::Required
            } else {
                ZeroCopy::Off
            },
        })
    })();
    let config = match parsed {
        Ok(config) => config,
        Err(problem) => {
            println!("kernel paths: bad configuration: {problem}");
            return;
        }
    };

    let measurements: Vec<Measurement> = match run_all(&config) {
        Ok(measurements) => measurements,
        Err(error) => {
            println!("kernel paths: failed: {error}");
            return;
        }
    };
    let baseline = measurements
        .iter()
        .find(|m| m.mode.contains("recvmmsg"))
        .map(Measurement::per_second);
    println!(
        "kernel paths on {} ({} generator threads, {:?} each)",
        config.interface, config.senders, config.duration
    );
    println!(
        "  {:<32} {:>13} {:>13} {:>12}  note",
        "path", "datagrams/s", "offered/s", "vs recvmmsg"
    );
    for m in &measurements {
        let offered = m.sent as f64 / m.elapsed.as_secs_f64();
        let ratio = baseline.map_or_else(|| "-".to_string(), |b| format!("{:.2}x", m.per_second() / b));
        let limited = if m.generator_limited() {
            "; generator-limited, a lower bound"
        } else {
            ""
        };
        println!(
            "  {:<32} {:>13.0} {:>13.0} {:>12}  {}{}",
            m.mode,
            m.per_second(),
            offered,
            ratio,
            m.note,
            limited
        );
    }
}

#[cfg(not(all(target_os = "linux", feature = "xdp")))]
fn kernel_paths() {
    println!();
    println!("kernel paths: not built; they need Linux and `--features xdp`");
}

fn main() {
    let fixture = fixture();
    let mut criterion = Criterion::default().configure_from_args();
    parsing(&mut criterion, &fixture);
    datagram_costs(&mut criterion, &fixture);
    criterion.final_summary();
    loopback_udp();
    kernel_paths();
}
