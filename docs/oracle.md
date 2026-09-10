# The oracle

A randomness beacon and a multi-signed price feed, and the host bindings that
let a contract read either.

- `vrf/` — RFC 9381 `ECVRF-EDWARDS25519-SHA512-TAI`, pinned to the RFC's vectors.
- `src/oracle/registry.rs` — who the chain believes.
- `src/oracle/beacon.rs` — the randomness accumulator.
- `src/oracle/feed.rs` — price records, the median, what an authority signs.
- `src/state/oracle_exec.rs` — execution against the block overlay.
- `src/core/oracle_payload.rs` — the wire forms.
- `vm/src/host.rs`, `vm/src/runtime.rs` — the contract-facing ABI.

---

## Read this part first

Three things about this subsystem are worse than the rest of the chain, and all
three are deliberate.

### 1. It introduces a trusted party

Everything else here is trustless in the strict sense: proof of work decides
ordering, signatures decide authority over funds, and a validator who lies is
caught by arithmetic anyone can redo.

An external price is a fact about the world. No amount of cryptography makes a
chain able to observe one. So the oracle names a set of authorities and believes
what a quorum of them agrees on — a genuine reduction in the security model,
bounded by a quorum requirement, a median, mandatory freshness, and rotation, but
not eliminated by any of them.

A network that does not want a trusted party sets `oracle: null` in its genesis
file, gets no `o:` records, and has the same state root it would have had before
this subsystem existed. **Introducing one is a decision somebody writes down, not
a consequence of upgrading.**

### 2. The VRF is a classical primitive on a post-quantum chain

`custom-l1-node` removed ed25519 from transaction authorization on purpose, so
that "an adversary who broke either scheme in isolation gets nothing." The VRF
reintroduces curve25519 — not for authorization, but for randomness.

There is no standardized, practical post-quantum VRF today. Two things bound the
cost:

- **A break costs future unpredictability, not past outputs.** An adversary who
  recovers a beacon key predicts randomness from that moment on. Randomness
  already consumed by a committed block is exactly as sound as it was.
- **Every key carries a scheme tag from the first block.** A post-quantum VRF
  lands as scheme 2 beside this one, exactly as SLH-DSA landed beside ML-DSA.
  Without the tag on day one, that migration is a hard fork of every stored key.
  It costs one byte.

### 3. A miner gets one bit of influence per block

The beacon is `beacon_n = H(beacon_{n-1} ‖ VRF(sk_proposer, beacon_{n-1} ‖ n))`.
The proposer cannot choose its output — a VRF is unique per (key, input) — and
cannot choose the input, both halves of which are fixed before it acts. The miner
cannot compute the output at all.

What the miner *can* do is include the proof or leave it out. Leaving it out
yields a deterministic fallback instead, which is a different value. **That is
one free bit of influence over every block a miner wins, retryable as often as it
wins one.**

Bounded, and not zero. The accumulator chains, so steering a value *k* blocks out
requires influence over all *k*; and the fallback is deterministic, so the choice
is between two known values rather than a search.

The fix, if that bit ever matters, is to move the proof into the block header, so
choosing between the two values costs a full re-mine. That changes the
proof-of-work preimage, the miner, the pool protocol, and the GPU kernel — which
is why it is named here rather than done.

**Do not use this beacon for a lottery whose payout exceeds a block reward.**

---

## Freshness is measured in blocks, not seconds

`src/state/context.rs` already says why, in the code: *"block timestamps are
miner-influenced within the consensus tolerance, so a timestamp-based deadline is
a deadline an adversary can nudge."*

It is worse than that. `chain.rs` reads `header.timestamp` **only** for
difficulty retargeting. There is no future-drift bound, no median-time-past, no
validity rule of any kind. A miner may write any `u64` it likes.

So a feed carries `updated_height` and nothing else, and `oracle_read` takes
`max_age_blocks`. A timestamp-based freshness check on this chain would read as
safety and provide none.

Making timestamps mean something is a separate, consensus-breaking change:
median-time-past over the last 11 blocks plus a maximum future drift. It is not
done here.

---

## The ABI makes forgetting impossible

```wat
(import "env" "block_randomness" (func $rand   (param i32)         (result i32)))
(import "env" "oracle_read"      (func $read   (param i32 i64 i32) (result i32)))
(import "env" "oracle_feed_age"  (func $age    (param i32)         (result i64)))
```

`oracle_read(feed_ptr, max_age_blocks, out_ptr)` returns `8` on success, `-1` for
an unknown feed, and `-2` for a stale one. **Nothing is written on failure**, so a
contract that ignores the return value reads whatever was already in its buffer
rather than a number that looks like a price.

The freshness bound is a *parameter*, not a field of the result. That is the
whole design. A contract cannot read a price without stating how stale a price it
will accept, so the failure mode most oracle post-mortems are made of — an author
who forgot to check the age — is unreachable rather than merely discouraged.

`-1` and `-2` are distinguished because they call for different handling: an
unknown feed is a bug in the contract, a stale one is a fact about the world that
may resolve on its own.

The randomness a contract sees is the **previous** block's, because the
accumulator folds after execution. A value available during execution is one the
block's own transactions could have been written against.

---

## Aggregation

**The median, not the mean.** One compromised authority moves a mean without
limit. It moves a median by nothing at all until it holds a majority of the
quorum. `one_liar_in_a_quorum_cannot_move_the_median` in `tests/oracle_tests.rs`
puts `u64::MAX` in a five-signer quorum and asserts the stored value is unmoved.

**No averaging on an even count.** The lower middle is taken. Averaging
introduces a division, a division introduces rounding, and a rounding rule in a
price is a systematic bias in whichever direction it falls.

**The quorum must be a strict majority.** Anything less admits two disjoint
quorums, and therefore two contradictory values for one round, each perfectly
valid.

**A quorum is a count of distinct authorities.** Without that check, one key
repeated three times clears a threshold of three.

### Check order

1. The feed exists.
2. The round is newer than the stored one — the replay guard.
3. Every signature verifies, from a registered authority, no authority twice.
4. The count clears the threshold.
5. Only then, the median.

Verification is step 3 on purpose. The three cheap rejections come first, so a
malformed submission costs a lookup rather than a quorum's worth of post-quantum
signature checks.

---

## What a feed costs

ML-DSA and SLH-DSA do not aggregate. There is no post-quantum analogue of a BLS
multisignature, so a submission carries one public key and one signature per
authority and grows linearly in the quorum.

| Quorum | Submission | Per 8 MiB block | Verification |
|---|---|---|---|
| 3 | 38.6 KiB | 212 | 0.9 ms |
| 5 | 64.3 KiB | 127 | 1.4 ms |
| 11 | 141.4 KiB | 57 | 3.1 ms |
| 21 | 269.9 KiB | 30 | 6.0 ms |

One observation is 13,157 bytes: a 1,984-byte hybrid public key beside an
11,165-byte hybrid signature, plus the eight-byte value. The key has to travel
because an address is a *hash* of a hybrid key, so a registry entry cannot yield
the key needed to check a signature.

The bytes bind before the CPU does. Reproduce with `cargo bench --bench oracle`.

---

## Bounds

| Constant | Value | What it stops | Measured |
|---|---|---|---|
| `MAX_AUTHORITIES` | 21 | A rotation quietly making every later feed update too large to gossip | 269.9 KiB per submission |
| `MIN_AUTHORITIES` | 3 | A "median" of two | — |
| `MAX_FEED_SUBMISSIONS_PER_BLOCK` | 8 | The most expensive verification work one block can demand | ~48 ms, 2.1 MiB |
| `MAX_OBSERVATIONS` | 21 | Signatures a node verifies before deciding to ignore them | 286 µs each |
| `MAX_HASH_TO_CURVE_ATTEMPTS` | 256 | An unbounded loop in consensus code | unreachable at ~2⁻²⁵⁶ |

VRF verification — paid by every node on every block, forever — is **206 µs**.
Proving is 196 µs, paid once per block by the proposer.

---

## Rotation

A change needs a quorum of the *current* set, the same threshold that moves a
price. There is no admin key: a separate rotation authority would be a single
point of failure sitting above a construction built to avoid one.

The approval commits to the incoming set **and the epoch**, and the epoch must be
exactly one past the current. Without that, an old approval could be replayed to
reinstate a set that was later rotated away from — the attack a rotation
mechanism invites if the epoch is left out of what is signed.

The incoming set is revalidated through `OracleRegistry::new`, so a rotation
cannot install a set the chain would have refused at genesis.

---

## Liveness

An absent or invalid beacon proof is **not an error**. The chain folds the
fallback and moves on.

Refusing blocks with no beacon proof would hand any single authority the power to
halt the chain by going offline — a worse property than the one bit of miner
influence it would buy back.

Feeds have the matching hazard in the other direction: if authorities stop
submitting, a feed goes stale and stays stale. That is why freshness is
mandatory at the ABI rather than advisory.

---

## Reorgs

Beacon and feed records live under the `o:` prefix and go through the same
generic overlay, undo journal, and root fold the trading subsystem uses. A
reverted block restores both.

It matters for the same reason it matters for pool reserves: a beacon left at the
abandoned chain's value is randomness nobody agreed to, and a feed left at the
abandoned chain's price is a lie that looks exactly like a fact.

---

## Why the VRF is its own crate

Not for the reason `ledger-math` and `dex` are crates — this one has a dependency
graph, so Kani cannot compile it and no proof harness is coming.

It is a crate because a beacon operator, the wallet, or a CLI needs to *produce*
proofs without linking RocksDB, libp2p, and the SNARK stack. A beacon that could
only run inside a full node would be a beacon nobody could operate.

### What pins it

RFC 9381's own test vectors, in `vrf/tests/rfc9381_vectors.rs`. This is not
ceremony. The first version of this implementation used suite octet `0x04`
instead of `0x03` — the octet belonging to `ECVRF-EDWARDS25519-SHA512-ELL2`, the
same curve and hash under a different hash-to-curve map. Every round trip passed.
Every proof it produced was self-consistent and would have been rejected by every
other implementation of the suite it claimed to be.

Consensus code with no second implementation on this chain to disagree with it
has to be checked from outside the repository.

The vectors' published intermediates — `x`, `H`, `k` — are checked separately in
`vrf/src/ecvrf.rs`, so a regression says *which stage* diverged rather than only
that something did.

---

## What is deliberately not built

**Timestamp validity rules.** See above. Height-based freshness is what this
chain can actually enforce today.

**A beacon proof in the block header.** The stronger fix for miner influence,
costed above and not taken.

**Contract-callable VRF.** A contract can read the beacon; it cannot ask for a
proof over an input of its own. That would need per-call VRF verification inside
the gas meter and a key the contract could name, neither of which exists.

**Any market feed wiring.** `src/rpc/market.rs` still returns 503 rather than
inventing a price. Feeding it from the oracle is a small change and is not part
of this work.
