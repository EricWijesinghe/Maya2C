# Block relay with an XDP/AF_XDP receive path

**Status: RESEARCH, off by default.** Node-local; changes no consensus rule.
Nothing in `bins/maya2c-node/src/main.rs` enables it: a node gets a relay only from
`Node::with_block_relay`, and the kernel path only on Linux with the `xdp`
feature. The program loads and passes `hal/ebpf-net/tests/xdp_veth.rs` against a
Linux 6.18 kernel (WSL 2) over veth, in generic mode; it has not run on a NIC
with native XDP.

Code:
- `hal/ebpf-net/common` — header, token bucket, verdict order, map records.
  `no_std`, dependency-free, compiled for the host and for the kernel.
- `hal/ebpf-net/programs` — the XDP program. Not a workspace member.
- `hal/ebpf-net/src` — sealing, chunking, reassembly, the receiver, and under
  `crates/node/src/linux/` the loader, the AF_XDP socket, and the throughput harness.
- `crates/node/src/network/relay_key.rs` — key exchange.
- `crates/node/src/network/node/relay.rs` — the driver side.

Tests:
- unit tests in each module;
- `hal/ebpf-net/tests/relay_tests.rs`;
- `hal/ebpf-net/tests/xdp_veth.rs` (Linux, root, ignored by default);
- `crates/node/tests/block_relay_tests.rs`;
- `fuzz/fuzz_targets/xdp_relay_decode.rs`.

Bench: `crates/node/benches/ebpf_bench.rs`.

## The brief, and what it maps to on this chain

| Asked | Built | Why the difference |
|---|---|---|
| An `ebpf_net` module using `aya` to compile and load eBPF/XDP **C** programs | The program is Rust (`aya-ebpf`), loaded by `aya`. The decision logic it calls lives in `maya-ebpf-net-common` and runs under the host test suite | aya loads ELF objects; it compiles nothing. C would need clang in the build and would duplicate the rules in a second language that no host test reaches |
| Inspect incoming P2P frames and drop malicious or malformed ones at the driver | A **separate UDP block relay** beside gossip. Its datagrams carry a fixed header the kernel can read. The program drops relay datagrams in three cases: the source is blocklisted, the source is over its token bucket, or the structure is wrong. It passes **everything else** | Every P2P byte is ciphertext (TCP → Noise → ML-KEM → yamux, `crates/node/src/network/pq/mod.rs`). No kernel program can see a frame's contents, so none can judge them |
| Route valid block transfers to user space, zero-copy via AF_XDP | One AF_XDP socket per queue. Zero-copy where the driver supports it, copy mode otherwise. Chunks are decrypted from UMEM into the reassembly buffer, one copy, which any reassembly needs | Zero-copy is a property of the driver; the code asks for it and reports what it got |
| Flatbuffers serialization, under 100 µs parsing at 100 Gbps | A fixed 56-byte header parsed by bounds-checked reads. Flatbuffers is the benchmark baseline | Three reasons, below the table |
| A benchmark proving 5× over TCP/UDP sockets | `crates/node/benches/ebpf_bench.rs` measures one `recv` per datagram, `recvmmsg` ×64, AF_XDP and `XDP_DROP`. It reports the generator's offered rate beside each figure, and **asserts no ratio** | A benchmark written to prove a number fixed in advance proves nothing. The ratio depends on the NIC, the driver and the attach mode |

Why no flatbuffers on the wire:
- **The kernel cannot run it.** The verifier accepts a packet read only after a bounds check, and flatbuffers' vtables make every field access a chain of offsets the verifier would have to follow.
- **It is not canonical.** Two encodings of one chunk would be two sets of AEAD associated data.
- **Parsing is not the cost.** A 56-byte parse takes nanoseconds, and the AEAD over 1,160 bytes dominates.

What a TCP comparison would compare: the gossip path is TCP plus two AEAD layers plus yamux, so a comparison against raw frames is not like for like. The bench's baseline is a UDP socket carrying the same datagrams.

## Wire format

```text
offset  len  field
     0    4  magic         "MY2R"
     4    1  version       1
     5    1  kind          1 = block chunk
     6    2  reserved      zero
     8    8  key_id        u64 BE, which relay key sealed the chunk
    16   32  block_id      the block's header id
    48    2  chunk_index   u16 BE
    50    2  chunk_count   u16 BE, always ceil(body_len / 1160)
    52    4  body_len      u32 BE, at most 8 MiB (the gossip ceiling)
    56    …  sealed chunk  plaintext + 16-byte tag
```

A datagram's exact length follows from its header, so a datagram one byte long
or short is refused without a key.

`CHUNK_LEN` is 1,160 so that a full datagram over IPv6 is exactly 1,280 bytes,
the IPv6 minimum MTU. That size is asserted at compile time.

`MAX_BODY_LEN` equals `MAX_GOSSIP_MESSAGE_BYTES`, also asserted at compile
time, in `crates/node/src/network/node/relay.rs`: a block too large to gossip must not be
deliverable some other way.

## Keys

`/maya/relay-key/1.0.0` runs over the libp2p connection the peers already share.
- **Request:** the peer with the lower `PeerId` sends 32 random bytes and a port.
- **Response:** the other peer answers with 32 bytes and a port.
- **Derivation:** both sides derive two directional keys from the two contributions and both peer ids, using BLAKE3 in derive-key mode.

Properties:
- **Confidentiality:** the key is as confidential as gossip. Recovering it means breaking X25519 *and* ML-KEM-768.
- **Authentication:** a chunk that opens came from the peer Noise authenticated.

Each chunk is XChaCha20-Poly1305. The nonce is derived from the whole header, and the header is the associated data. A nonce repeats only when the header repeats. An honest sender's plaintext for that header is then identical, because a block id commits to its transactions (invariant 24).

**A peer names a port, never an address.** Datagrams go to the IP its connection came from. Otherwise a peer could aim a node's block bursts at a victim, and the node would be the amplifier. A connection with no IP, such as the memory transport, gets no relay.

The contributions pass through the CBOR codec, whose buffers are not zeroized. The derived keys are.

## The kernel program

For each frame:
1. Anything that is not certainly IPv4 or IPv6 UDP addressed to the relay port gets `XDP_PASS`. That includes VLAN-tagged frames, IPv6 extension headers and later fragments. A node's SSH and libp2p traffic are never touched.
2. **Blocklist.** A blocked source spends no tokens, so its flood cannot evict honest buckets from the LRU map.
3. **Token bucket,** per source. It runs *before* the structural check, so garbage costs the sender tokens exactly as valid chunks do. The default burst is two maximal blocks.
4. **Structure:** fields, and the exact length they imply.
5. **Redirect** to the AF_XDP socket on the frame's queue. If no socket is bound there, the frame is dropped, never passed.

Steps 2–4 are `verdict::judge`, the function the host tests exercise.
`hal/ebpf-net/tests/xdp_veth.rs` and the fuzz target's agreement property hold the program to it.

### What the verifier refused, and the rules that came out of it

The first three loads against a real kernel were refused. Each refusal is now
a rule written down in the code at the place it applies.

1. **"invalid access to packet".** LLVM saw that an earlier check (the IP total
   length fits the packet) implied a later read was in bounds, and deleted that
   read's own bounds check. The verifier cannot follow the implication through
   a scalar, so it refused the read. *Rule:* every packet read comes before any
   length field is compared with the packet (`read`, `fits` in `main.rs`).
2. **"R11 is invalid".** `verdict::judge` takes six arguments, and a BPF call
   passes five in registers. Emitted as a real call, the sixth went through
   R11. *Rule:* shared functions the program calls are `#[inline(always)]`.
3. **"R9 !read_ok".** In a separate function built from `RelayHeader::decode`,
   LLVM read a callee-saved register it treated as undefined while assembling
   the returned `Result`. *Rule:* the same inlining, applied to every function
   on the kernel's path (`maya-ebpf-net-common` crate docs).

None of the three shows up in `cargo check` or in any host test.

**The licence string is not GPL-compatible, deliberately.** The program uses only helpers that are not GPL-only. If someone adds a GPL-only helper (`aya-log`'s perf output is one), the load fails and the kernel names the helper. The alternative is that the object quietly changes licence.

## Blame

Only authenticated evidence blames anyone.

**Kernel blocklist entries come only from peer-guard quarantines,** which rest on authenticated connections. A UDP source address is whatever the sender wrote, so blocklisting on relay traffic would let anyone get an honest peer's address blocked. Entries expire in the kernel as well as being lifted from user space, so a crash cannot leave a peer blocked until reboot.

When several quarantined peers share an address, the block lasts as long as the longest of their quarantines. It is lifted only when none remain. An honest peer behind the same NAT as a quarantined one loses the **relay** for that time. It keeps gossip, which the blocklist does not touch.

In user space:

| Outcome | Blames |
|---|---|
| Malformed datagram, unknown key, failed AEAD | nobody (the key id is public) |
| No room to reassemble | nobody |
| Two chunks at one index with different bytes, or chunks disagreeing about length | the key's owner, as `MalformedFrame` |
| A body whose header id is not the block id its chunks named | the sender, as `MalformedFrame` |
| A body contradicting its `tx_root`, or undecodable | the sender, exactly as gossip would: it goes through `gossiped_block` |

Reassembly is per `(sender, block_id)`. Deduplicating across senders would let one peer relay garbage under a real id and suppress every honest relay of that block. Memory limits:
- 4 incomplete blocks and 16 MiB per sender;
- 256 MiB in total;
- a 10 s timeout for an incomplete block.

These limits belong to the node, not to each queue. Every AF_XDP worker shares
one reassembler, because a sender can vary its UDP source port to spread chunks
across RSS queues. Separate reassemblers would give it a fresh allowance on each
queue.

The key exchange is bounded too. A peer that asks again within 10 s of its last
exchange gets no offer, and a response that arrives after its peer was
quarantined installs nothing.

## What it does not do

- **Forward.** A node relays blocks it publishes, not blocks it receives. Gossip carries blocks onward after validating them.
- **Transmit through AF_XDP.** Sending uses a kernel UDP socket on the blocking pool.
- **Configure RSS.** `queues` must cover every queue relay traffic can land on. Traffic on an unserved queue is dropped and counted as `redirect_failed`.
- **Log from worker threads anywhere visible.** They use `log`, and the node installs no logger. Failures on the driver side surface as `NodeEvent::RelayFailure`.

## `unsafe`

`hal/ebpf-net/src/linux/` and `hal/ebpf-net/programs` carry `unsafe` by construction:
- UMEM and the rings are memory shared with the kernel;
- the program reads packets through pointers the verifier bounds.

Both are listed with the other exemptions in `docs/architecture-vision.md` §7. Every block carries a `// SAFETY:` note (`scripts/check-unsafe.sh`). Without the `xdp` feature the node links none of it.

## Building and running

```text
# the program, on Linux or Windows: install the prebuilt bpf-linker v0.11.1
# (github.com/aya-rs/bpf-linker/releases). `cargo install bpf-linker` needs a
# system libLLVM of rustc's exact LLVM version and fails without one.
cd hal/ebpf-net/programs && cargo build --release
# -> hal/ebpf-net/programs/target/bpfel-unknown-none/release/maya-relay-xdp

# the kernel tests
sudo -E MAYA_XDP_OBJECT=... cargo test -p maya-ebpf-net --features xdp \
  --test xdp_veth -- --ignored --test-threads=1

# the benchmark
sudo ./scripts/xdp_netns.sh up
sudo -E MAYA_XDP_BENCH=1 MAYA_XDP_OBJECT=... cargo bench --features xdp --bench ebpf_bench
```

Privileges needed:
- **Program:** `CAP_BPF` and `CAP_NET_ADMIN`.
- **AF_XDP sockets:** additionally `CAP_NET_RAW`.

Measured figures are in [benchmarks/ebpf-net.md](benchmarks/ebpf-net.md), with the machine they came from.
