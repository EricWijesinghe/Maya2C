# Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s transaction authorization

Every transaction on this chain carries two signatures over the same bytes: a
lattice proof under **FIPS 204** (ML-DSA-65) and a hash-based proof under
**FIPS 205** (SLH-DSA-SHA2-128s). Both must verify. Neither is a fallback for
the other, and there is no wire version, configuration flag, or legacy path that
accepts one.

This document explains why, what it cost, and the one design decision that would
have silently made the whole thing worthless.

---

## Why two post-quantum schemes

ML-DSA-65 is already post-quantum, so a second post-quantum scheme looks
redundant. It is not, because the two are post-quantum for unrelated reasons.

**ML-DSA rests on Module-LWE** — a structured lattice assumption roughly fifteen
years old. It is believed hard. It is not *proven* hard, and the algebraic
structure that makes it fast (the ring, the NTT) is also the structure a future
attack would exploit. A cryptanalytic break of Module-LWE would forge every
transaction this chain has ever accepted.

**SLH-DSA rests on nothing but SHA-2** — preimage and collision resistance, no
algebraic structure to attack, and a security argument that predates the chain
by decades. If SHA-2 falls, far more than this chain is already lost.

The two therefore fail independently. A forgery needs to break lattices *and*
hashes, not either one. That is the only thing that makes the cost below worth
paying.

---

## The decision that makes it real

**The address must commit to both public keys.** This is the part that is easy
to get wrong, and getting it wrong makes the entire feature decorative.

Suppose the address kept committing only to the ML-DSA key, as v2 did. An
adversary who broke Module-LWE could forge the lattice half for any account —
and then simply *generate their own SLH-DSA keypair*, sign with it, and attach
it to the transaction. Both checks pass. The hash-based half would have added
7856 bytes and zero security, because nothing pinned it to the victim.

So:

```
address = blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)
```

and `Transaction::signing_bytes` commits to both keys, so each signature covers
the *other* scheme's key. Neither half can be lifted out of one transaction and
replayed beside a partner key an attacker chose.

`crates/node/tests/hybrid_tests.rs` asserts this from four angles — a good lattice proof
beside a bad hash proof, the mirror case, a key pair spliced from two accounts,
and a bit-flip sweep over every byte of the hash key's contribution to the
address.

### Consequence: this is a hard fork

v2 addresses are hashes of an ML-DSA key alone and have no v3 preimage. There is
no migration, and deliberately no compatibility path: accepting a v2 address
would mean accepting a single-signed transaction, which is the exact thing this
change exists to prevent.

- Genesis allocations must be regenerated (`l1-wallet address`).
- The genesis chain-id domain moved to v2 so an old and a new node cannot agree
  on a genesis hash while disagreeing about who owns the premine.
- Keystores moved to version 4. Version 3 holds a valid ML-DSA key and still
  cannot be upgraded: pairing it with a fresh SLH-DSA key names a different
  address holding nothing. `l1-wallet` says so rather than failing obscurely.
- Wire versions 1–4 are refused by name. 3 and 4 carry a perfectly good ML-DSA
  signature and are refused anyway.

---

## Why the `s` parameter set

FIPS 205 offers "small" and "fast" variants. Measured side by side:

| | SHA2-128s | SHA2-128f |
|---|---|---|
| signature | 7856 B | 17088 B |
| sign | ~196 ms | ~9 ms |
| verify | ~0.22 ms | ~0.56 ms |

`f` signs twenty times faster, which looks decisive until you ask who pays.
Signing happens **once**, in a wallet, per transaction. Verification happens on
**every node, for every transaction, in every block, forever** — and `s`
verifies 2.6× faster while producing a signature less than half the size. For a
replicated ledger the asymmetry runs the opposite way from most systems: the
one-time cost is the cheap one.

---

## What it cost

From `cargo bench --bench hybrid_signing` and `cargo bench --bench hybrid_footprint`:

| | ed25519 | ML-DSA-65 | hybrid |
|---|---|---|---|
| public key | 32 B | 1952 B | 1984 B |
| signature | 64 B | 3309 B | 11165 B |
| one-output transfer | ~200 B | ~5.3 KB | **13215 B** |
| transfers per 8 MiB gossip message | ~40000 | 1574 | **634** |
| channel closure | 216 B | 10610 B | **26386 B** |
| keygen | 15 µs | 0.14 ms | 14 ms |
| sign | 26 µs | 0.22 ms | **105 ms** |
| verify | 15 µs | 0.099 ms | **0.18 ms** |
| 64-tx block, `apply_block` | — | — | **26 ms** |

### Verification barely moved

0.18 ms against 0.099 ms. A *rejected* signature costs 0.070 ms, because
`HybridVerifyingKey::verify` short-circuits at the lattice half — a node spends
one verification on junk rather than two, which matters because an attacker
chooses the rate at which junk arrives. Block validation was never the
constraint and still is not.

### Bytes and signing moved a great deal

`MAX_BATCH_CLOSURES` stays at 128 and is now the **binding** constraint rather
than a generous one: a full batch is ~3.2 MiB of the 8 MiB gossip ceiling, so
two such transactions fit in a block and a third does not. It was held at 128
rather than cut because the hundred-channel batch the L2 scale tests settle is a
property the system claims.

`TOTAL_TRANSFERS` in `crates/l2-flash/tests/scale_tests.rs` fell from 1000 to 20. Every
transfer is signed by both parties, so 1000 transfers is 2000 signatures — five
minutes per shape, six minutes for the file, long enough that `cargo test` reads
as hung rather than slow. The property under test does not depend on the volume;
the size assertions are what pin it. The full-volume run stays reachable under
`cargo test -- --ignored` and takes roughly an hour.

---

## Why `maya-crypto-pq` is a separate crate

`slh-dsa` is generic over its parameter set, so its hot code monomorphizes into
whichever crate *instantiates* it. A `[profile.dev.package.slh-dsa]` override —
the trick that keeps `fips204` usable in dev builds — therefore does nothing: it
optimizes a crate that contains none of the work.

Measured on a dev build:

| | sign |
|---|---|
| `[profile.dev.package.slh-dsa] opt-level = 3` | **4468 ms** |
| the same, plus the instantiating crate optimized | **140 ms** |

A 32× swing. So the instantiation is confined to `crates/crypto-pq/`, behind a
concrete, non-generic API, and the root `Cargo.toml` optimizes *that* crate in
dev. This keeps the node's own code unoptimized and debuggable, which is the
property the existing profile overrides exist to preserve — raising
`[profile.dev]` workspace-wide would have sacrificed it.

---

## Layout

| File | Role |
|---|---|
| `crates/crypto-pq/src/lib.rs` | Concrete SLH-DSA-SHA2-128s wrapper; the only crate that instantiates `slh-dsa` |
| `crates/node/src/crypto/keys.rs` | The ML-DSA-65 half. No `address()` — a lattice key does not name an account on its own |
| `crates/node/src/crypto/hybrid.rs` | `HybridSignature`, `HybridPublicKey`, the key types, and the v3 address |
| `crates/node/src/core/transaction.rs` | Wire versions 5/6, `signing_bytes` over both keys, `txid` over both signatures |
| `crates/node/src/core/payload.rs` | `ChannelClosure` with two key pairs and two signature pairs |
| `crates/node/src/state/db.rs` | `stage_transaction` — the both-or-reject rule at block execution |
| `crates/node/src/network/mempool.rs` | The same rule, applied earlier |
| `crates/node/src/state/settlement.rs` | Closure verification, hash-pin before signatures |
| `crates/node/benches/hybrid_signing.rs` | Time: per scheme, per operation, and through `apply_block` |
| `crates/node/benches/hybrid_footprint.rs` | Bytes: counting global allocator, static and peak |
| `crates/node/tests/hybrid_tests.rs` | The dual-signing rule and the address binding |
| `crates/node/tests/malleability_tests.rs` | Encoding surface of both schemes |

---

## What this does not cover

The shielded pool still proves joinsplits with Groth16 over BLS12-381, a
pairing-based system whose soundness rests on discrete log. A quantum adversary
who cannot forge a transfer under this scheme can still forge a shielded proof
and mint hidden supply. Replacing that proof system is separate work, and until
it happens the chain's post-quantum security is the weaker of the two halves.
