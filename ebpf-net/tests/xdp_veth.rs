//! The XDP program and AF_XDP path against a real kernel, over a veth pair.
//!
//! This is what holds the kernel program to the rules the host tests pin in
//! `maya-ebpf-net-common`: `packet::parse` and `verdict::judge` are tested on
//! the host, but the program reaches the same fields with pointer arithmetic,
//! and only a kernel can say whether the two agree.
//!
//! Needs Linux, root (or `CAP_BPF`, `CAP_NET_ADMIN`, `CAP_NET_RAW`,
//! `CAP_SYS_ADMIN` for namespaces), `ip`, and the compiled object:
//!
//! ```text
//! cd ebpf-net/programs && cargo build --release && cd ../..
//! sudo -E MAYA_XDP_OBJECT=$PWD/ebpf-net/programs/target/bpfel-unknown-none/release/maya-relay-xdp \
//!   cargo test -p maya-ebpf-net --features xdp --test xdp_veth -- --ignored --test-threads=1
//! ```
//!
//! Ignored by default, so `cargo test --workspace` on a Linux machine without
//! root does not fail for reasons that are not bugs.

#![cfg(all(target_os = "linux", feature = "xdp"))]

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use maya_ebpf_net::common::Counter;
use maya_ebpf_net::common::header::CHUNK_LEN;
use maya_ebpf_net::common::rate::RateLimit;
use maya_ebpf_net::linux::baseline::enter_netns;
use maya_ebpf_net::{
    AttachMode, BlockSealer, KeyBook, RelayEvent, RelayKey, Role, XdpIngress, XdpIngressConfig,
    ZeroCopy, derive_keys,
};

const PORT: u16 = 30_334;
const WAIT: Duration = Duration::from_secs(5);

/// A veth pair, one end in its own namespace, torn down on drop.
struct Veth {
    namespace: String,
    host: String,
    host_addr: Ipv4Addr,
    peer_addr: Ipv4Addr,
}

fn ip(args: &[&str]) {
    let status = Command::new("ip").args(args).status().expect("run ip");
    assert!(status.success(), "ip {args:?} failed");
}

impl Veth {
    fn create(tag: u8) -> Self {
        let veth = Self {
            namespace: format!("maya-xdp-test{tag}"),
            host: format!("mxh{tag}"),
            host_addr: Ipv4Addr::new(10, 202, tag, 1),
            peer_addr: Ipv4Addr::new(10, 202, tag, 2),
        };
        let peer = format!("mxp{tag}");
        let host_cidr = format!("{}/24", veth.host_addr);
        let peer_cidr = format!("{}/24", veth.peer_addr);
        // A previous run that panicked may have left these behind.
        let _ = Command::new("ip")
            .args(["link", "del", &veth.host])
            .status();
        let _ = Command::new("ip")
            .args(["netns", "del", &veth.namespace])
            .status();

        ip(&["netns", "add", &veth.namespace]);
        ip(&[
            "link", "add", &veth.host, "type", "veth", "peer", "name", &peer,
        ]);
        ip(&["link", "set", &peer, "netns", &veth.namespace]);
        ip(&["addr", "add", &host_cidr, "dev", &veth.host]);
        ip(&["link", "set", &veth.host, "up"]);
        ip(&[
            "netns",
            "exec",
            &veth.namespace,
            "ip",
            "addr",
            "add",
            &peer_cidr,
            "dev",
            &peer,
        ]);
        ip(&[
            "netns",
            "exec",
            &veth.namespace,
            "ip",
            "link",
            "set",
            &peer,
            "up",
        ]);
        veth
    }

    /// Sends each datagram to `port` on the host end, from inside the namespace.
    fn send(&self, port: u16, datagrams: Vec<Vec<u8>>) {
        let namespace = self.namespace.clone();
        let target = (self.host_addr, port);
        std::thread::spawn(move || {
            enter_netns(&namespace).expect("enter namespace");
            let socket = UdpSocket::bind("0.0.0.0:0").expect("bind sender");
            for datagram in datagrams {
                socket.send_to(&datagram, target).expect("send");
                // Paced, so a test measures the program and not the veth queue.
                std::thread::sleep(Duration::from_micros(200));
            }
        })
        .join()
        .expect("sender thread");
    }
}

impl Drop for Veth {
    fn drop(&mut self) {
        let _ = Command::new("ip")
            .args(["link", "del", &self.host])
            .status();
        let _ = Command::new("ip")
            .args(["netns", "del", &self.namespace])
            .status();
    }
}

fn object() -> String {
    std::env::var("MAYA_XDP_OBJECT").expect("MAYA_XDP_OBJECT must name the compiled program")
}

struct Harness {
    veth: Veth,
    ingress: XdpIngress,
    events: mpsc::Receiver<RelayEvent<u8>>,
    sender_key: RelayKey,
}

fn harness(tag: u8, limit: RateLimit) -> Harness {
    let veth = Veth::create(tag);
    let keys = KeyBook::shared();
    let theirs = derive_keys(&[1; 32], &[2; 32], b"peer", b"node", Role::Requester);
    let ours = derive_keys(&[1; 32], &[2; 32], b"peer", b"node", Role::Responder);
    keys.write().unwrap().insert(tag, ours.inbound);

    let mut config = XdpIngressConfig::new(veth.host.clone(), object(), PORT);
    config.attach = AttachMode::Generic;
    config.zero_copy = ZeroCopy::Off;
    config.limit = limit;
    let (sender, events) = mpsc::channel();
    let ingress = XdpIngress::start(config, keys, move |event| {
        let _ = sender.send(event);
    })
    .expect("start the XDP ingress");
    Harness {
        veth,
        ingress,
        events,
        sender_key: theirs.outbound,
    }
}

fn sealed(key: &RelayKey, body: &[u8]) -> Vec<Vec<u8>> {
    let sealer = BlockSealer::new(key, [0xAA; 32], body).unwrap();
    (0..sealer.chunk_count())
        .map(|index| {
            let mut out = Vec::new();
            sealer.seal(index, &mut out).unwrap();
            out
        })
        .collect()
}

fn counter(harness: &Harness, which: Counter) -> u64 {
    harness.ingress.counters().expect("counters")[which.slot() as usize]
}

fn wait_counter(harness: &Harness, which: Counter, at_least: u64) -> u64 {
    let deadline = Instant::now() + WAIT;
    loop {
        let value = counter(harness, which);
        if value >= at_least || Instant::now() >= deadline {
            return value;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "needs Linux, root, and MAYA_XDP_OBJECT"]
fn a_sealed_block_reaches_user_space_and_other_traffic_still_passes() {
    let harness = harness(1, RateLimit::DEFAULT);
    let body: Vec<u8> = (0..CHUNK_LEN * 3 + 11).map(|i| i as u8).collect();
    let datagrams = sealed(&harness.sender_key, &body);
    let count = datagrams.len() as u64;

    let bystander = UdpSocket::bind((harness.veth.host_addr, PORT + 1)).expect("bind bystander");
    bystander.set_read_timeout(Some(WAIT)).unwrap();

    harness.veth.send(PORT, datagrams);
    match harness.events.recv_timeout(WAIT) {
        Ok(RelayEvent::Block {
            peer, body: got, ..
        }) => {
            assert_eq!(peer, 1);
            assert_eq!(got, body);
        }
        other => panic!("expected the block, got {other:?}"),
    }
    assert!(wait_counter(&harness, Counter::Redirected, count) >= count);

    harness
        .veth
        .send(PORT + 1, vec![b"not relay traffic".to_vec()]);
    let mut buffer = [0u8; 64];
    let len = bystander
        .recv(&mut buffer)
        .expect("non-relay UDP must pass");
    assert_eq!(&buffer[..len], b"not relay traffic");
    assert!(counter(&harness, Counter::Passed) >= 1);
}

#[test]
#[ignore = "needs Linux, root, and MAYA_XDP_OBJECT"]
fn malformed_and_blocked_datagrams_are_dropped_in_the_kernel() {
    let mut harness = harness(2, RateLimit::DEFAULT);
    let good = sealed(&harness.sender_key, b"a small block");

    let mut bad_magic = good[0].clone();
    bad_magic[0] = b'X';
    let mut one_byte_long = good[0].clone();
    one_byte_long.push(0);
    harness.veth.send(PORT, vec![bad_magic, one_byte_long]);
    assert!(wait_counter(&harness, Counter::Malformed, 2) >= 2);

    let peer = IpAddr::V4(harness.veth.peer_addr);
    harness
        .ingress
        .block(peer, Instant::now() + Duration::from_secs(60))
        .expect("block");
    harness.veth.send(PORT, good.clone());
    assert!(wait_counter(&harness, Counter::Blocked, 1) >= 1);
    assert!(
        harness
            .events
            .recv_timeout(Duration::from_millis(500))
            .is_err()
    );

    harness.ingress.unblock(peer).expect("unblock");
    harness.veth.send(PORT, good);
    assert!(matches!(
        harness.events.recv_timeout(WAIT),
        Ok(RelayEvent::Block { .. })
    ));
}

#[test]
#[ignore = "needs Linux, root, and MAYA_XDP_OBJECT"]
fn a_source_over_its_rate_is_dropped_in_the_kernel() {
    let limit = RateLimit {
        burst: 10,
        per_second: 0,
    };
    let harness = harness(3, limit);
    let one = sealed(&harness.sender_key, b"x");
    harness
        .veth
        .send(PORT, std::iter::repeat_n(one[0].clone(), 50).collect());
    assert!(wait_counter(&harness, Counter::RateLimited, 40) >= 40);
    assert!(counter(&harness, Counter::Redirected) <= 10);
}
