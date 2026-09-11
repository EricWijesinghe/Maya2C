# Stratum V2 for Maya2C

The pool protocol takes Stratum V2's *architecture* and carries a mining
sub-protocol shaped around Maya2C's header. It is not wire-compatible with stock
SV2 clients, and this document is mostly about why that is a decision rather
than a shortfall.

Implemented: [`stratum-v2/`](../stratum-v2) — framing, messages, codec — and
[`pool-service/`](../pool-service), the daemon that joins them to chain types:
share validation, the PPLNS ledger, and treasury payouts.
[`pool-service.md`](pool-service.md) documents that half; this document is the
protocol underneath it.

One message was added after this document was first written.
`SubmitWorkerTelemetry` (`0x22`, in SV2's unused `SubmitSolution` slot) carries
a rig's self-reported power, temperature, and fan speed. It is listed in §3's
table of deviations for completeness and discussed properly in
[`pool-service.md`](pool-service.md#8-rig-telemetry-is-self-reported-and-cannot-be-checked):
nothing in it is verifiable, and the pool keeps it structurally out of payout
arithmetic for that reason.

---

## 1. Why interoperability is not on the table

Stratum V2's mining messages assume a Bitcoin header. They carry a
`merkle_root`, a compact `nbits`, a rollable `version`, and a coinbase
transaction to hide an extranonce in.

Maya2C's header is 144 bytes and keeps only the root, as `tx_root`
([`src/core/block.rs:9-41`](../src/core/block.rs)):

```text
prev_hash[32] ‖ state_root[32] ‖ timestamp[8] ‖ nonce[8] ‖ difficulty_target[32] ‖ tx_root[32]
```

No stock SV2 client — SRI, Braiins, anything else — could mine this chain even
against a byte-perfect implementation of the specification. It would need an
ArgonBlake hasher and a Maya2C header builder, and at that point it is a
different client that happens to share a framing layer.

Given that, claiming extension type `0x0000` would announce a compatibility that
does not exist, and the failure mode is bad: a stock client would parse our
`NewMiningJob` as its own and mine garbage rather than report an error. So
Maya2C's messages live under extension `0x4D41` (`"MA"`), and a stock client's
frames are refused with `UnknownExtension`.

**Message numbering still tracks the specification slot for slot.** Anyone
holding the SV2 spec can read a hex dump of this protocol.

## 2. What is taken unchanged

The frame header, exactly as specified — six little-endian bytes:

```text
┌────────────────┬──────────┬────────────┬──────────────────┐
│ extension_type │ msg_type │ msg_length │ payload          │
│ U16            │ U8       │ U24        │ msg_length bytes │
└────────────────┴──────────┴────────────┴──────────────────┘
  bit 15 of extension_type is the channel_msg flag
```

Nothing in it mentions a block header, so there was nothing to adapt. Also taken
unchanged: the channel abstraction, per-channel targets, batched share
acknowledgement, and the separation of template choice from pool operation —
which is the actual point of SV2.

## 3. Deviations, and what forced each

| SV2 | Here | Why |
|---|---|---|
| `merkle_root` in `NewMiningJob` | `tx_root` **and** `state_root` | The header commits to the transaction tree, as in SV2, and also to the post-execution state, which the chain checks |
| `nbits` (U32 compact) | `target` (32 bytes) | Targets are full 256-bit values compared bytewise ([`src/crypto/pow.rs:16-18`](../src/crypto/pow.rs)); there is no compact form to pack into |
| `version` in jobs and shares | *absent* | The header has no version field to roll |
| `SetExtranoncePrefix` | `SetNonceRange` | No coinbase means no extranonce; see below |
| `ntime` (U32) | `timestamp` (U64) | The header's timestamp is a `u64` |
| `U256` little-endian | 32 bytes in header order | See below |
| `SubmitSolution` (`0x22`) | `SubmitWorkerTelemetry` | No job declaration means no solution is ever submitted apart from a share, leaving the slot free for rig health — which SV2 has nowhere to put |

### Nonce ranges instead of extranonces

In Bitcoin each miner gets a distinct search space by varying the coinbase
extranonce. Maya2C has no coinbase, so the only field a miner may vary is the
8-byte `nonce` — and with tens of thousands of connections on one job, every one
of them would otherwise start at zero and walk the same path.

Each channel is assigned a disjoint half-open nonce range at open time, the way
[`src/consensus/miner.rs:6-10`](../src/consensus/miner.rs) already partitions
across threads. Three things follow: two channels cannot collide by construction
rather than by luck; duplicate detection becomes exact rather than probabilistic;
and a nonce outside a channel's range is rejected without hashing anything —
which, at 25 ms per verification, is the cheapest rejection the pool has.

### Targets are not byte-swapped

SV2 declares 32-byte fields `U256` and little-endian. Maya2C compares targets as
big-endian byte strings and stores hashes in that order. Swapping on the wire
would mean every target crossing this boundary needed a swap back before
`meets_target` could look at it, and one missed swap yields a target wrong by a
factor of 2²⁵⁶ that still looks like a plausible 32-byte value. These are
carried exactly as the header holds them.

## 4. Share validation is the scaling wall

Not the connection count. 50,000 Tokio connections is unremarkable; **each share
costs a 25.4 ms Argon2id pass over 32 MiB**
([`src/crypto/argon_blake.rs:16-22`](../src/crypto/argon_blake.rs)) — six orders
of magnitude more than Bitcoin's two SHA256d.

| Shares per connection | Shares/sec at 50k | CPU cores of pure Argon2 |
|---|---|---|
| 1 / sec | 50,000 | 1,270 |
| 1 / 10 s | 5,000 | 127 |
| 1 / 60 s | 833 | 21 |
| 1 / 10 min | 83 | 2.1 |

Three design consequences:

- **Vardiff is the load governor, not a courtesy.** `SetTarget` is what keeps
  aggregate share rate near 1/60 s per channel. That is the difference between
  21 cores and 1,270.
- **Validation never runs on a connection task.** Shares go over a bounded
  channel to a validator pool sized to cores, and backpressure sheds load by
  *raising* targets rather than dropping shares.
- **The GPU miner is the validator.** [`cuda-miner`](../cuda-miner)'s batch fill
  is exactly a batch share validator — same lane layout, same batching.

A fourth consequence from `TARGET_BLOCK_TIME = 15`s
([`src/consensus/difficulty.rs:32`](../src/consensus/difficulty.rs)): jobs go
stale fast. Job push has to be sub-second and the stale-grace window measured in
hundreds of milliseconds, not the seconds a ten-minute chain can afford.

## 5. There is no block reward

`grep -i 'coinbase|subsidy|reward'` across `src/` returns nothing. `Block` is
`{header, transactions}` with no coinbase field, and `apply_block_checked`
stages only the block's own transactions — no subsidy is credited to anyone.
Transaction fees go to `FEE_SINK = [0u8; 32]`
([`src/state/shielded.rs:342`](../src/state/shielded.rs)), an address with no
private key that [`src/rpc/market.rs:47`](../src/rpc/market.rs) excludes from
circulating supply. Fees are **burned**, not paid to miners.

A pool splits a block reward, and there is not one. The payout ledger is built
anyway, because the accounting is correct under any reward model:

- **Share credits** accrue in the immutable ledger now, denominated in share
  weight rather than coin.
- **Settlement** happens through ordinary signed transfers from a pool treasury
  account, which the chain already supports with no consensus change.
- **A coinbase or block subsidy** would make mining self-sustaining. That is a
  hard fork — it changes `apply_block_checked` and the supply schedule — and it
  is deliberately out of scope here rather than smuggled in.

That ledger now exists, and the consequence shows up as a startup check rather
than as a footnote: `pool-service` has **no default** for what a found block
distributes, and refuses to run until an operator sets one. There is no honest
default to pick — the number is policy funded from a treasury, not a constant
the chain could supply.

## 6. Transport

The pool reuses [`src/network/pq/`](../src/network/pq): Noise XX over X25519
with an ML-KEM-768 exchange layered inside, ChaCha20Poly1305, 4-byte length
prefixes, 64 KiB maximum plaintext.

SV2 specifies `Noise_NX_secp256k1_ChaChaPoly_SHA256` with authority-signed
certificates. Adopting it would introduce secp256k1 — the one classical
primitive this codebase deliberately avoids, having gone ML-DSA-65 + SLH-DSA for
signatures and layered ML-KEM into transport specifically against
harvest-now-decrypt-later
([`src/network/pq/handshake.rs:26-29`](../src/network/pq/handshake.rs)) — to buy
interop that §1 shows is unreachable. Peer authentication uses ML-DSA, like the
rest of the chain.

`MAX_PAYLOAD_LEN` is set from the transport's 64 KiB ceiling rather than from the
`U24` field's 16 MiB. A frame larger than the carrier can deliver is not a frame,
and accepting a declared length above it would let a peer make the pool reserve
for a message that can never arrive — with 50,000 connections, that is the whole
attack.

## 7. Decoding discipline

Copied from [`src/core/codec.rs`](../src/core/codec.rs) rather than reinvented,
because these bytes arrive from whatever dialled the mining port:

- Every read is range-checked. A truncated frame is an error, never a panic: a
  pool that aborts on a malformed frame is a pool one connection can take down.
- A declared length above `MAX_PAYLOAD_LEN` is refused before anything is
  reserved. At this connection count a length field is an allocation primitive.
- `Reader::finish` rejects trailing bytes, so one message cannot have two
  encodings. For a share submission that is concrete: two encodings would be two
  share identities for one piece of work, and the duplicate check keys on
  decoded fields.
- Non-finite `f32` hash rates are rejected rather than clamped. A NaN propagates
  through every vardiff comparison that touches it, and the comparison that would
  normally catch a bad value returns false for NaN.

`fuzz/fuzz_targets/sv2_frame_decode.rs` covers `Frame::decode` and
`Message::from_frame`, asserting canonicality and length honesty. Its seeds are
generated through the real encoders by `cargo run --example gen_fuzz_corpus`,
for the reason [`fuzz/README.md`](../fuzz/README.md) gives.
