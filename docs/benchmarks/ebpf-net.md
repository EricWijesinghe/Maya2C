# Block relay benchmark (`benches/ebpf_bench.rs`)

Recorded 2026-09-15 on one machine, Intel Core Ultra 9 275HX:

- **Linux:** WSL 2, kernel 6.18.33 (`microsoft-standard-WSL2`), 24 vCPUs, the VM capped at 12 GB. `cargo bench --features xdp`, release profile, criterion with 1 s warm-up and 3 s measurement. The kernel paths ran over the veth pair from `scripts/xdp_netns.sh`, as root.
- **Windows 11:** the same bench without the kernel paths. It ran while the machine was short of commit memory, and its user-space figures came out roughly twice the Linux ones. The Linux figures are the ones to use.

**Not measured: a NIC.** veth runs XDP in generic mode and AF_XDP in copy mode, and both ends share one kernel and one set of cores. The figures below are a property of that path, not of a 100 GbE NIC with native XDP and zero-copy.

## Parsing

| Measurement (Linux) | Time per datagram | Within 100 µs by |
|---|---:|---:|
| `RelayHeader::split` (56-byte fixed header, all rules) | 3.12 ns | ~32,000× |
| `verdict::judge` (the kernel's decision: blocklist, bucket, header) | 2.54 ns | ~39,000× |
| flatbuffers `root` verification, the same fields, the same rules | 74.4 ns | ~1,340× |

Both formats clear the brief's 100 µs parsing target by three orders of
magnitude or more, so that target constrains nothing. The fixed header is 24×
faster than verified flatbuffers. That speed is not why it was chosen: the XDP
verifier cannot walk flatbuffers' offset tables at all.

## Per-datagram cost in user space

| Measurement (Linux) | Time | Throughput, one core |
|---|---:|---:|
| Seal a 1,160-byte chunk (XChaCha20-Poly1305 + nonce derivation) | 1.89 µs | 585 MiB/s |
| Open a 1,160-byte chunk | 1.76 µs | 629 MiB/s |
| The cipher alone over the same 1,160 bytes, built once, fixed nonce | 1.74 µs | 635 MiB/s |
| Ingest a 1 MiB block (904 datagrams: parse, key lookup, open, reassemble) | 1.80 ms (1.99 µs per datagram) | 557 MiB/s |
| Loopback UDP into the full receiver, syscalls included | 2.4 µs | 424,084 datagrams/s |

**The AEAD is the cost, and the relay adds almost nothing to it.** Opening a
chunk costs 1.76 µs and the bare cipher 1.74 µs. Key lookup, nonce derivation,
header checks and reassembly together are about 1%.

**What this means at 100 Gbps.** A line of 1,232-byte relay datagrams is about
9.7 million a second: 1,294 bytes each on the wire, counting Ethernet, IPv4,
UDP, preamble and gap. At 1.76 µs to open one, decryption alone needs roughly
17 cores. The receive path — XDP, AF_XDP, zero-copy — decides how cheaply a
datagram reaches user space. It does nothing about the AEAD.

At blocks every 15 seconds and at most 8 MiB each, a relay's steady load is
under 0.6 Mbit/s. The relay's case is burst latency and dropping floods cheaply,
not sustained throughput.

## Kernel paths (veth, generic XDP, copy-mode AF_XDP)

1,232-byte structurally valid relay datagrams from 12 generator threads in the
peer namespace; 5 s per path; one receive queue.

| Path | Datagrams/s | Offered/s | vs `recvmmsg` | Note |
|---|---:|---:|---:|---|
| Kernel UDP, one `recv` per datagram | 2,392,643 | 11,148,021 | 0.97× | no program |
| Kernel UDP, `recvmmsg` ×64 | 2,468,597 | 9,887,567 | 1.00× | no program; also the user-space drop baseline |
| AF_XDP, queue 0 | 1,616,937 | 1,618,056 | 0.66× | generic, copy; **generator-limited** |
| `XDP_DROP`, blocklist | 13,743,722 | 13,743,721 | 5.57× | generic; **generator-limited** |

Reading the table:

- **Dropping in XDP clears the brief's 5× — on the drop path, not the receive
  path.** A blocklisted flood is refused at 13.7 M datagrams/s against 2.47 M
  for a batched socket that receives and discards. That figure is a lower
  bound, because the program kept up with everything the generator offered.
- **AF_XDP receive is not faster here, and this setup cannot show whether it
  would be.** With the program attached, veth's generic XDP slows the sender,
  and the generator offered only 1.6 M/s. The receiver took all of it. In
  generic mode and copy mode the frame is still copied, which removes the
  saving AF_XDP exists for.
- **The receive comparison belongs to a NIC.** Native XDP and zero-copy are
  what this measurement cannot provide.

The first attempt at this table failed and found a bug in `ebpf-net/src/linux/xsk.rs`.
The kernel publishes its fill-ring consumer index in batches, so the ring can
show less room than the frames just received, and the receive loop treated that
as an error. It now refills what fits and keeps the rest for the next pass.

## Still to be measured

The same table on a NIC with native XDP and zero-copy AF_XDP (mlx5, ice, i40e),
with the NIC, driver, kernel version, queue count and attach mode recorded
beside every figure.
