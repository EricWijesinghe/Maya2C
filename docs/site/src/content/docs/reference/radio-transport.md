---
title: 'Off-Grid Radio Transport'
editUrl: false
# GENERATED from docs/radio-transport.md by scripts/ingest.mjs. Edit the source, not this.
---
Moving Maya2C headers where there is no IP.

- `hal/radio-transport/src/frame.rs` — the on-air frame, and why AX.25's shape on ISM's rules.
- `hal/radio-transport/src/duty.rs` — the governor that refuses rather than warns.
- `hal/radio-transport/src/fountain.rs` — loss answered by sending more, not by asking again.
- `hal/radio-transport/src/relay.rs` — custody, expiry, and why a relay never looks inside.
- `crates/node/src/network/radio_gateway.rs` — the node side, and why it is a gateway and not a libp2p transport.
- `crates/node/tests/radio_transport_tests.rs`, `fuzz/fuzz_targets/radio_frame_decode.rs`.

---

## The one idea to read first

**This carries headers. It does not carry transactions, and the reason is
arithmetic rather than preference.**

| Object | Size |
|---|---:|
| `BlockHeader` | 144 B |
| `AccountProof` | ~600 B – 1 KB |
| `HYBRID_SIGNATURE_LENGTH` | **11,165 B** |
| One transaction | ~13 KB |

Every Maya2C signature is a hybrid pair — ML-DSA-65 plus SLH-DSA — and both
halves must verify, so there is no smaller signature to send. Those bytes are
also incompressible: FIPS 204 already bit-packs `z`, `h` and `c` at their
information-theoretic width, and SLH-DSA is a tree of hash outputs. A compressor
applied to either produces output **larger** than its input.

Against that, the link. A full SF12 frame is 2,465,792 µs of airtime — the
figure every LoRaWAN calculator prints, and what `duty.rs` computes to the
microsecond. At EU868's 1% duty cycle that is one frame per ~246 seconds, and an
hour of spectrum is **fourteen frames**.

So: a header is one frame at SF7 and six at SF12. A transaction is 60 frames at
SF7 — about forty minutes — and 259 at SF12, which is most of a day. The
subsystem is a header relay, exactly as `docs/architecture-vision.md` §3 scoped
it, and `light-client` is what makes a header useful on the far end.

---

## The rule that governs all of it

From that same section:

> None of them may change consensus. A block is a block regardless of the medium
> that carried it, and the chain must never acquire a rule that depends on *how*
> a message arrived — that would make the transport layer consensus-critical and
> hand an attacker a fork by radio.

Nothing in the crate inspects what it carries. The relay forwards opaque
bundles, the fountain codes opaque bytes, the frame wraps an opaque payload.
`a_relay_carries_a_payload_it_cannot_interpret` hands the store bytes that are
not a header, not a transaction, not anything, and asserts it carries them —
because a relay whose idea of "valid" filtered traffic would be a relay an
attacker can teach what to drop.

---

## ISM, not amateur

The framing is AX.25's — address pair, control byte, protocol id, payload,
checksum — because every packet-radio tool in existence can already decode it,
and being able to point a TNC at a link is worth more than three saved bytes.

The **spectrum** is ISM (868/915 MHz), and that is load-bearing. Amateur
allocations forbid encrypted transmission in most jurisdictions, and Maya2C's
transport is ML-KEM-768 over Noise. Running on amateur bands would mean dropping
the encryption to be legal, and stamping an operator's callsign — required there
— on every relayed frame, which turns a relay mesh into a map of who is running
one. So: AX.25's shape, ISM's rules, two-byte node tags that identify nobody.

---

## The governor refuses; it does not warn

A duty-cycle limit is a condition of using the spectrum without a licence, not a
performance target. A governor that logged a warning and transmitted anyway
would make its operator an unlicensed transmitter, and they would find out from
a regulator rather than from a log.

So `DutyCycle::reserve` returns an error. There is no override, no `force`, and
no configuration that raises the budget above the band's — a knob that can be set
to 100% is the failure the module exists to prevent.

Airtime is **computed, not measured**: it is a function of spreading factor,
bandwidth, coding rate and length, all known before transmission, so the
governor can refuse before the radio keys up. Measuring afterwards would produce
an audit log of violations. The arithmetic is integer microseconds throughout —
two nodes disagreeing by a rounding step is two nodes disagreeing about whether
a transmission was legal, and `f64` would put a rounding mode in the middle of
that.

---

## Why a fountain code, and which one

A 1% duty cycle means a lost frame does not cost a retransmission — it costs
*another window*, ~246 seconds at SF12. And half these links are one-way: a node
on a hilltop transmitting to whoever can hear it has nobody to take a NACK from.

So the sender never learns what was lost. It emits symbols until it stops, and a
receiver that collects any `k + ε` recovers the object.

### It is a random linear fountain, and that was measured

The first implementation was an LT code with an ideal-soliton degree
distribution — the textbook answer. At 30% loss it recovered **42 of 60**
channels *even sending 160% overhead*, because the soliton's tail is tuned for
`k` in the thousands and these objects are a few hundred blocks at most. A block
of headers is small by design, and small is where LT is weakest.

A random linear fountain has the opposite trade: `O(k²)` decoding instead of
`O(k log k)`, but recovery at `k + ~10` symbols whatever `k` is. At `k ≤ 1024`
the quadratic cost is about a megabit of word operations — nothing beside 246
seconds of silence per frame. The expensive resource is airtime, so the code
spends the cheap one.

Sweeping the replacement over sixty seeded trials at 30% loss:

| overhead | recovered |
|---:|---:|
| 60% | 58 / 60 |
| 80% | 60 / 60 |
| 100% | 60 / 60 |

`OVERHEAD_PERCENT` is 80, set from that table rather than guessed. Sixty is
where the arithmetic says it should sit (`1.1k / 0.7 ≈ 1.57k`) and also where
two channels in sixty fail; the extra twenty buys the tail. A receiver one
symbol short waits another window.

An earlier revision also removed the degree-1 spike on the theory that the
systematic prefix already supplies single blocks. Belief propagation can only
*start* from a symbol with one unknown, so with no degree-1 symbols at all,
2,900 coded symbols solved zero blocks of a hundred. That failure is recorded in
the source, because the reasoning that produced it is plausible.

---

## Why a gateway and not a libp2p transport

The plan for this was a `TransportKind::Radio` beside `Memory` and `Tcp`. The
link budget says it should not be.

A libp2p connection opens with multistream-select, a Noise handshake and a yamux
negotiation. Maya2C's Noise is ML-KEM-768: a 1,184-byte encapsulation key and a
1,088-byte ciphertext, before protocol negotiation and before one byte of
payload. At SF12 that handshake is **hours**, per connection, to move a 144-byte
header.

A transport that cannot complete its own handshake inside the useful lifetime of
what it carries is not a transport. So the radio path is a gateway: headers in,
fountain symbols out, symbols in, headers out. No session, no negotiation, no
per-peer state that has to survive a week of intermittent contact. It is the
shape every working LoRa mesh converges on.

---

## Store-and-forward

There is no path to compute. A node in a valley hears two neighbours, one of
whom walks over a ridge each week, and somewhere past that is a node with IP
transit. No routing protocol converges on that, because the links are never
simultaneously up. What works is the postal model: hold a message, hand it to
whoever you meet.

`a_bundle_crosses_two_hops_that_are_never_up_at_once` is that premise as a test:
sender and receiver are never in contact, and the relay meets each at a
different time.

Three details that are decisions rather than defaults:

- **Dedup is by payload digest, recomputed on decode.** A bundle's id would be
  chosen by whoever made it, so deduplicating on it would let one sender claim
  another's key and suppress that bundle everywhere it reached — censorship for
  the price of one frame.
- **Eviction is by expiry, then by distance travelled.** A bundle that has
  crossed six hops has had six chances; a fresh one has had none. Dropping the
  newest would make a busy relay a black hole for everything arriving during the
  busy period.
- **The seen-set ages out.** A digest remembered forever is a slow leak on a node
  that runs for years, and the reason to remember one expires with the bundle.

---

## Two defects the tests found

Both were in the gateway, and both would have looked fine in review.

**The sender restarted at symbol zero every window.** A rateless code's whole
advantage is that the next symbol is as good as the last, so a sender interrupted
by the duty cycle must resume with *new* symbols. Without a cursor, an SF12
object — twelve windows' worth — re-sent the same fourteen frames forever.

**The receiver re-decoded completed objects.** A fountain sender cannot know when
to stop, having no return path, so spare symbols keep arriving after a receiver
has enough. The gateway started a fresh decode for each: sixteen headers arrived
**112 times**. That is also a work bound — symbols come from anyone with a
transmitter, and a receiver that re-decoded on demand would do unbounded Gaussian
elimination for the price of repeating somebody else's frames.

---

## Status

**RESEARCH.** Nothing in consensus calls it. The gateway is constructed by
nothing in `crates/node/src/bin/`, there is no serial device behind `RadioLink`, and a chain
with no gateway produces the state root it would have had without the subsystem.

Promoting it means, in order: a real `RadioLink` over a serial port behind a
feature, a decision about which node originates header batches and how often,
and a field trial — the duty-cycle arithmetic is exact, but the propagation is
not something a test can tell you about.
