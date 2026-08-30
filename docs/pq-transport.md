# Post-quantum P2P transport (FIPS 203 / ML-KEM-768)

Every libp2p connection this node makes carries an ML-KEM-768 key exchange
layered above its Noise session, and every byte above that layer is sealed with
ChaCha20-Poly1305 under a key derived from it.

```
TCP / Memory
  └── libp2p-noise   Noise_XX_25519_ChaChaPoly_SHA256   ← unchanged, audited
        └── /maya/mlkem/1.0.0                           ← this work
              ML-KEM-768 (FIPS 203) + ChaCha20-Poly1305
                └── yamux → gossipsub
```

---

## What this protects, and what it does not

**Protected: harvest now, decrypt later.** An adversary recording gossip traffic
today and waiting for a cryptographically relevant quantum computer must break
**ML-KEM-768 *and* X25519** to read any of it. This is the threat that actually
applies to a chain — gossip reveals which peer originated which transaction, and
that metadata does not become less interesting with age.

**Not protected: live impersonation.** Peer authentication stays classical. The
peer is authenticated by the Noise layer below, against its ed25519 `PeerId`,
and nothing here changes that. An adversary holding a quantum computer *at the
moment a connection is made* could still impersonate a peer. That is an active
attack requiring the machine to exist and be on the wire — much harder than
recording packets — but it is a real limit. Closing it means post-quantum peer
identity, which changes what a `PeerId` is, and is separate work.

Stating this plainly matters more than the feature does. "Post-quantum
transport" that quietly means "post-quantum confidentiality only" is the kind of
claim that gets repeated without its caveat.

---

## Why a layer, and not a post-quantum Noise pattern

The obvious implementation is `Noise_XXhfs_25519+MLKEM768_ChaChaPoly_SHA256`.
It is not reachable:

- `libp2p-noise 0.46.1` hard-codes its parameters in a private `PARAMS_XX`
  behind a private resolver. `Config` exposes `new`, `with_prologue`, and
  `with_webtransport_certhashes` — no way to change the pattern or the key
  exchange.
- One layer down, `snow 0.9.6` *does* expose a `Kem` trait under its `hfs`
  feature. But `snow::params::KemChoice` is a **closed enum with exactly one
  variant, `Kyber1024`**, so a pattern string naming ML-KEM-768 does not parse.

An ML-KEM implementation could be registered under the name `Kyber1024` and both
ends are ours, so it would interoperate. **That was rejected.** A handshake whose
advertised protocol name misstates its primitive is a trap for whoever audits it
next.

That left three options: fork two security-critical crates, write a full
authenticated key exchange from scratch, or layer. Layering leaves the audited
Noise handshake untouched and adds a small, self-contained stage above it. The
handshake here is *not* an AKE and does not try to be — it runs inside a channel
Noise has already authenticated and encrypted.

---

## Why `ml-kem`, not `pqcrypto-kyber`

`pqcrypto-kyber 0.8.1` was last published **2024-01-25**. FIPS 203 was finalized
in **August 2024**. That crate implements **round-3 Kyber**, and round-3 Kyber768
is *not* ML-KEM-768 — FIPS 203 changed the shared-secret KDF (the ciphertext
hash was dropped), changed encodings, and added the seed-based key format.
Shipping it under a FIPS 203 label would be false.

| Crate | Version | FIPS 203? | Pure Rust? |
|---|---|---|---|
| `pqcrypto-kyber` | 0.8.1 (2024-01) | **No — round-3 Kyber** | No (PQClean C FFI) |
| `pqcrypto-mlkem` | 0.1.1 (2025-08) | Yes | No (PQClean C FFI) |
| **`ml-kem` (RustCrypto)** | **0.3.2** | **Yes** | **Yes** |

Pure Rust also matters here for the same reason it did for `fips204` and
`slh-dsa`: the Dockerfile's static-musl build links only RocksDB's C++, and
PQClean bindings would add a C toolchain and an unsafe FFI surface to it.

---

## The exchange

Two fixed-width messages, inside the established Noise session:

```
responder → initiator   [1 byte version][1184 byte encapsulation key]
initiator → responder   [1 byte version][1088 byte ciphertext]
```

Both sides then derive directional keys with BLAKE3, over the shared secret
**and the full transcript**:

```
okm = BLAKE3-derive_key(ctx, secret ‖ domain ‖ version ‖ ek ‖ ct)
initiator→responder = okm[0..32]
responder→initiator = okm[32..64]
```

Two keys, never one: a single key in both directions would put the two peers'
nonce counters in the same space, and a repeated `(key, nonce)` pair destroys
ChaCha20-Poly1305 outright. Separate keys make that structurally impossible
rather than something the framing code has to remember.

### Implicit rejection

ML-KEM decapsulation **never fails**. On a forged ciphertext FIPS 203 returns a
pseudorandom value derived from the ciphertext and a per-key rejection seed. So
nothing in this code branches on decapsulation "succeeding" — a mismatch surfaces
as the two sides deriving different keys and the first frame failing to
authenticate. Decoding a received *encapsulation key* can fail, and does: that
is FIPS 203's modulus check, and a peer can send anything.

### Framing

```
[4 bytes big-endian length][ciphertext, tag included]
```

Length covers ciphertext and tag, so a frame is self-delimiting and a truncated
one fails to authenticate rather than being accepted short. Every length is
checked against a 64 KiB + 16 ceiling **before a byte is allocated**, mirroring
the discipline in `src/core/codec.rs`. Nonce counters are per-direction and
refuse to wrap — at `u64::MAX` the connection dies rather than repeating a nonce.

---

## No downgrade

The upgrade is not optional. It offers exactly one protocol, so a peer that does
not speak `/maya/mlkem/1.0.0` fails multistream-select and the connection is
dropped. There is deliberately no fallback: an optional post-quantum layer is one
an attacker strips.

---

## Rotation: post-compromise security, not forward secrecy

The request was "key rotation every 1,000 blocks to enforce PFS". Two
corrections, both load-bearing:

**Noise XX already provides PFS**, and so does this layer — the ML-KEM keypair is
generated fresh per connection and dropped when the handshake ends. An adversary
who seizes a node tomorrow cannot decrypt traffic recorded today. PFS was never
missing.

**What rotation adds is post-compromise security.** A node in a quiet mesh can
hold a connection open for days; without rotation one handshake would protect all
of it, and a key lifted from a live process would unlock all of it. Forcing a
re-handshake every epoch caps that window.

### The epoch is local, and never negotiated

An earlier design carried the epoch in the handshake and had both peers agree.
That does not survive a real network: peers sit at different heights, a syncing
node is thousands of blocks behind, and a fresh node has no chain at all. Two
honest peers would compute different epochs and fail to connect — a self-inflicted
partition, exactly when the network can least afford one.

So the epoch never goes on the wire. Each node notes the epoch a connection was
established in and closes it once its own epoch moves on. libp2p redials, the
upgrade runs again, fresh keys are in place. Whichever side notices first drives
it; the other sees a reconnect. Disagreement means one peer rotates sooner than
the other, which is harmless.

**The `PeerId` does not rotate.** Rotating the libp2p identity would invalidate
every bootnode address, Kademlia routing entry, and gossipsub peer score every
~4.2 hours, and the DHT would never converge.

The clock takes the **later** of height-based and wall-clock epochs, so neither
can stall rotation: a node with a frozen height still rotates on time, and a node
that syncs a year of history in ten minutes rotates on height.

---

## Measured cost

`cargo bench --bench mlkem_handshake`:

| | |
|---|---|
| generate keypair | 49 µs |
| decode peer key + encapsulate | 45 µs |
| decapsulate | 49 µs |
| **full connection handshake** | **106 µs** |
| handshake bytes on the wire | **2272 B** |
| frame sealing | **1.17 GiB/s** |
| frame opening | 1.89 GiB/s |
| 845 KB block, sealed | 674 µs |

Against a 15 s target block time a handshake is 0.0007 % of one block interval,
and one core sustains ~9,400 of them per second. Sealing a full 64-transaction
block costs under a millisecond. Neither number is close to mattering.

---

## Layout

| File | Role |
|---|---|
| `crypto-pq/src/kem.rs` | Concrete ML-KEM-768 wrapper; the only crate instantiating `ml-kem` |
| `src/network/pq/handshake.rs` | The two-message exchange and transcript-bound KDF |
| `src/network/pq/stream.rs` | Framed ChaCha20-Poly1305 duplex |
| `src/network/pq/rotation.rs` | `EpochClock` — when a session is stale |
| `src/network/pq/mod.rs` | The libp2p `InboundConnectionUpgrade` / `OutboundConnectionUpgrade` |
| `src/network/node.rs` | Upgrade applied to both transports; rotation sweep in the driver |
| `src/network/sim.rs` | `DelayStream` — latency injection for simulation |
| `tests/pq_transport_tests.rs` | Upgrade applied, no downgrade, rotation fires and heals |
| `tests/latency_sim_tests.rs` | Five-node propagation under 0 / 25 / 100 / 250 ms |
| `benches/mlkem_handshake.rs` | Handshake and sealing cost |

`maya-crypto-pq` holds the instantiation for the same reason it holds `slh-dsa`:
`ml-kem` is generic over its parameter set, so a `[profile.dev.package.ml-kem]`
override optimizes a crate containing none of the work. Measured on a dev build,
a handshake cost **5805 µs** with that override and **785 µs** once the
instantiating crate was optimized instead.

---

## Operating it

Three metrics, sampled by the exporter rather than pushed from the swarm task:

- `maya_pq_sessions_established_total` — completed ML-KEM handshakes.
- `maya_pq_sessions_rotated_total` — sessions closed for age.
- `maya_pq_rotation_epoch` — the epoch this node believes it is in.

Rotations should track roughly `peers × epochs elapsed`. Establishments rising
while rotations stay flat means rotation is not firing; the reverse means
sessions are being torn down by something other than the sweep.

Height is fed to the rotation clock by its own always-on task, not by the metrics
loop — a node started without `--metrics-addr` must still rotate on the schedule
its operator was told about.
