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
the discipline in `crates/node/src/core/codec.rs`. Nonce counters are per-direction and
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
| `crates/crypto-pq/src/kem.rs` | Concrete ML-KEM-768 wrapper; the only crate instantiating `ml-kem` |
| `crates/node/src/network/pq/handshake.rs` | The two-message exchange and transcript-bound KDF |
| `crates/node/src/network/pq/stream.rs` | Framed ChaCha20-Poly1305 duplex |
| `crates/node/src/network/pq/rotation.rs` | `EpochClock` — when a session is stale |
| `crates/node/src/network/pq/mod.rs` | The libp2p `InboundConnectionUpgrade` / `OutboundConnectionUpgrade` |
| `crates/node/src/network/node.rs` | Upgrade applied to both transports; rotation sweep in the driver |
| `crates/node/src/network/sim.rs` | `DelayStream` — latency injection for simulation |
| `crates/node/tests/pq_transport_tests.rs` | Upgrade applied, no downgrade, rotation fires and heals |
| `crates/node/tests/latency_sim_tests.rs` | Five-node propagation under 0 / 25 / 100 / 250 ms |
| `crates/node/benches/mlkem_handshake.rs` | Handshake and sealing cost |

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

---

# The second KEM: `/maya/dualkem/1.0.0` (draft FIPS 207 / HQC-192)

**Status: shipped disabled.** `DualKemPolicy::default()` is `Disabled`, so a
stock node offers only `/maya/mlkem/1.0.0` and nothing above changes.

## Draft, not standard

NIST **selected** HQC in March 2025 as the backup KEM to ML-KEM. The
specification is **draft FIPS 207** — not a published standard, and its
parameters and encodings can still move.

This matters because of the precedent set above: `pqcrypto-kyber` was rejected
for shipping round-3 Kyber under a FIPS 203 label, and that was called false.
Shipping a draft implementation as a standard would be the same error with a
longer fuse. So every name says draft, the crate is pinned exactly, and the
protocol carries its own version byte.

## Why a second KEM

ML-KEM rests on Module-LWE, a structured lattice assumption. HQC rests on
decoding random quasi-cyclic codes. The assumptions are unrelated, so an advance
against one is not an advance against the other — the same argument
`docs/hybrid-signatures.md` makes for ML-DSA plus SLH-DSA on every transaction,
applied to the transport.

## Confidentiality is an OR; availability is an AND

The point to weigh before enabling it.

The channel stays secret if **either** KEM holds. But the connection **works**
only if both implementations are correct: the session key is derived from both
secrets, so a panic or a mis-derivation in either one breaks every connection
that negotiated this protocol.

`ml-kem` is 0.3.2 against a final FIPS 203. `hqc-kem` is `0.1.0-rc.0` against a
draft. Pairing them gates the transport's liveness on the less-settled of the
two, to defend its secrecy against a break in the more-settled one. That can be
the right trade; it is not a free one, and it is why this is negotiated rather
than required.

## Why negotiated, and what the downgrade costs

`/maya/dualkem/1.0.0` is a **different protocol name**, not a version bump of
`/maya/mlkem/1.0.0`. libp2p's multistream-select offers both, so a peer that
speaks only the single-KEM protocol still connects. A version bump would have
made the old protocol unspeakable and forced a flag day on a network with no way
to coordinate one.

The cost is a downgrade an active attacker can force by suppressing the dual
protocol from negotiation. `DualKemPolicy::Required` closes that and exists for
when the conditions below hold.

### What has to be true before the default flips

1. `hqc-kem` reaches a non-release-candidate version.
2. Draft FIPS 207 is published final, or the draft is stable across at least one
   revision.
3. The protocol has run enabled across a non-trivial fraction of the network,
   through real churn, with no handshake failure attributable to the HQC half.

## Turning it on

```
node --dual-kem off        # default: offer /maya/mlkem/1.0.0 only
node --dual-kem preferred  # offer both, dual first
node --dual-kem required   # offer only /maya/dualkem/1.0.0
```

`disabled`, `offered`, `on`, `none`, and `strict` are accepted as aliases, and
the value is case-insensitive. The node prints the policy at startup
unconditionally — including `off` — because an operator debugging a failed
connection needs to know which side offered what, and an absent line is a worse
answer than "off".

**There is no config-file field.** This node has no `config.toml` layer at all:
it is configured by CLI flags plus `genesis.json`, and there is no `toml`
dependency in the tree. Adding one purely to hold this flag would be a larger
change than the flag, so `--dual-kem` is the whole configuration surface.

### Which pairs can connect

| Dialer \ Listener | off | preferred | required |
|---|---|---|---|
| **off** | ML-KEM | ML-KEM | **refused** |
| **preferred** | ML-KEM | dual | dual |
| **required** | **refused** | dual | dual |

The two refusals are structural rather than a check: a `required` node and an
`off` node offer disjoint protocol sets, so multistream-select finds nothing in
common and the connection drops. `crates/node/tests/dualkem_negotiation_tests.rs` pins every
cell of that table.

The `preferred` row is what makes incremental rollout possible — an operator can
enable it on one node without partitioning it — and it is also the downgrade an
active attacker can force by stripping the dual protocol from negotiation.
`required` is the answer to that, once the rollout conditions above hold.

### A flush bug this work surfaced

`dual::write_all` originally omitted the `flush()` that the single-KEM helper
performs. `AsyncWriteExt::write_all` fills a buffer; it does not put bytes on
the wire, and libp2p's noise writer emits a frame on flush or when its buffer
fills. The peer is blocked in `read_exact` for an exact byte count, so an
unflushed write is a deadlock.

The single-KEM handshake's 1,185-byte message survived that omission. The dual
handshake's 5,699-byte message did not — and the symptom was every
dual-negotiating pair failing to connect while every fallback pair succeeded,
which reads like a negotiation bug and is not one.
`the_handshake_survives_a_ring_smaller_than_its_first_message` is the regression
test.

## The combiner

```
okm = BLAKE3-derive_key(ctx_dual,
        ss_mlkem ‖ ss_hqc ‖ domain ‖ version
        ‖ ek_mlkem ‖ ct_mlkem ‖ ek_hqc ‖ ct_hqc)
initiator→responder = okm[0..32]
responder→initiator = okm[32..64]
```

**A KDF, not double encryption.** Encrypting twice — once under each secret —
would double the AEAD cost on every frame, double the nonce surface, and has no
standard security argument. One KDF over both secrets gives "secure if either
holds" outright, with the per-frame cost unchanged.

**Both transcripts, not just the secrets.** A combiner over shared secrets alone
is IND-CCA robust only if both KEMs are ciphertext-collision-resistant, which
neither specification promises. Binding both ciphertexts removes the assumption.
The single-KEM handshake already bound its transcript, so this inherits the
property rather than inventing it.

**A different context string.** If both protocols derived under the same `ctx`,
a dual session and an ML-KEM-only session sharing an ML-KEM transcript would
derive related keys — the second KEM contributing nothing at exactly the moment
it was meant to matter.

## Sizes, and a correction

| | ML-KEM-768 | HQC-192 |
|---|---|---|
| encapsulation key | 1,184 B | 4,514 B |
| ciphertext | 1,088 B | 8,978 B |

| | Wire total | Largest message | Segments @1460 | Fits initial window? |
|---|---|---|---|---|
| `/maya/mlkem/1.0.0` | 2,274 | 1,185 | 1 | yes |
| `/maya/dualkem/1.0.0` | 15,766 | 5,699 | 4 | yes |

The design discussion for this work assumed the dual handshake would spill past
the initial congestion window and cost an extra round trip: 15,766 / 1,460 is
eleven, and the window is ten. **That arithmetic is wrong.** Congestion control
is per-direction, so what must fit is the largest single message — 5,699 bytes,
four segments — not the two-direction total. Both handshakes fit, and neither
pays an extra round trip.

The extra bytes are still real and still cost bandwidth on every connection. They
do not cost latency. `network::pq::measure::exceeds_initial_window` takes the
largest message specifically so the mistake is hard to repeat.

## Measured cost

`crates/node/tests/dualkem_latency_tests.rs`, in-process transport with simulated one-way
delay, dev profile:

| One-way delay | single-KEM | dual-KEM |
|---|---|---|
| 0 ms | 6.1 ms | 9.6 ms |
| 10 ms | 22.0 ms | 35.0 ms |
| 50 ms | 57.8 ms | 70.4 ms |

There is **no sub-100 ms assertion** in that file, deliberately. On an
in-process transport both handshakes are fast enough that a 100 ms threshold
would pass forever — including after a change that tripled the cost. On a real
link the figure is dominated by round trips, which belong to the path rather
than to this code. The tests assert that both sides agree at every latency, and
print the cost for a human.

### The dev-profile override is load-bearing

`[profile.dev.package.hqc-kem]`, with `sha3` and `shake`, at `opt-level = 3`.

HQC-192 decapsulation runs Reed-Muller and Reed-Solomon decoders over ~18 kbit
codewords. Unoptimized, a dual handshake measured **66.6 ms**; with the override,
**9.6 ms**. Without it the transport tests read as broken rather than slow —
the same reason invariant 5 protects the `fips204` and `ark-*` overrides.

## An asymmetry worth knowing

FIPS 203 requires a modulus check on a received ML-KEM encapsulation key, so a
malformed one is refused at the handshake. **Draft FIPS 207 defines no
structural validity condition on an HQC encapsulation key**, so `hqc-kem` checks
the length and nothing else, and any correctly-sized string is accepted.

Not a vulnerability: encapsulating to a garbage key yields a secret the peer
cannot reproduce, the combined key differs, and the first frame fails to
authenticate. Just later and quieter than ML-KEM's rejection.
`HqcError::MalformedEncapsulationKey` is therefore currently unreachable, and
`hqc::tests::any_correctly_sized_string_is_accepted_as_an_encapsulation_key`
pins the fact so a future draft revision that adds a check is noticed.

## Evaluation of the implementation

`hqc-kem 0.1.0-rc.0` was selected over three alternatives:

| Crate | Verdict |
|---|---|
| `pqcrypto-hqc 0.2.2` | **Rejected** — PQClean C FFI, which the static-musl build and `check-unsafe.sh` exist to avoid |
| `pqc-kem 0.2.0` | **Rejected** — its `hqc` feature is `dep:oqs`, i.e. liboqs, the same objection wearing a Rust name |
| `lib-q-hqc 0.0.11` | Deprioritised — 0.0.x, pulls a whole framework, ships debug-interop features |
| `backbone-hqc 0.2.0` | Viable second choice — pure Rust, Apache-2.0 |
| **`hqc-kem 0.1.0-rc.0`** | **Selected** |

It is RustCrypto — the same organisation as the `ml-kem` already in the tree —
pure Rust, `no_std`, MIT OR Apache-2.0. Its 300 KAT vectors across NIST levels
1/3/5 were **run and pass**, against reference commit 161cd4f. Its dudect-style
constant-time tests were **run and pass**: decapsulation valid-vs-corrupted
Welch |t| = 0.15 and 0.72 against a 4.5 leak threshold, keygen |t| = 0.17. Its
only `unsafe` is a PCLMULQDQ carry-less multiply with a scalar fallback — a
constant-time primitive, and outside the first-party code `check-unsafe.sh`
gates.

The constant-time run is a smoke test on a loaded developer machine, not a
proof.
