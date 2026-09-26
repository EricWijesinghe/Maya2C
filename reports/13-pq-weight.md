# 13 — The post-quantum weight problem

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 13
Hardware record: [12-baseline.md](12-baseline.md).

> DONE WHEN: before/after bytes per finalized transaction, bandwidth per
> validator at each TPS level, and storage per day, all with hardware
> records; key registration and rotation tests pass; certificate aggregation
> choice justified by measurements in an ADR; DA withholding test passes in sim/.

## 1. Bytes per transaction

Measured from the wire: 400 signed v5 transfers = 5,286,000 B, which is
**13,215 B each** (`apply_pipeline` output). By the layout in `spec/01-encoding.md`:

| Part | Bytes | Share |
|---|---|---|
| signatures (ML-DSA-65 3,309 + SLH-DSA-128s 7,856) | 11,165 | 84.5 % |
| public keys (1,952 + 32) | 1,984 | 15.0 % |
| payload (one output 40 + nonce 8) | 48 | 0.4 % |
| framing (version, two counts, signature flag) | 18 | 0.1 % |

### Before and after

| Form | Bytes | Source |
|---|---|---|
| **Before:** v5 hybrid transfer, keys on the wire | 13,215 | measured |
| Hybrid with the public key replaced by a 32-byte key hash | 11,263 | arithmetic (13,215 − 1,984 + 32); not built for the hybrid frame |
| **After:** smart-account transfer, ML-DSA-65, key hash on the wire | **3,465** | `crates/smart-account` test `a_transfer_carries_a_key_hash_not_a_public_key` (vs 5,417 with the key) |

The "after" row does two things at once. It keeps the key off the wire, and it
signs with ML-DSA-65 alone, dropping the SLH-DSA half. The second part is a
security trade: the hybrid exists so that breaking one scheme is not enough.
Taking keys off the wire alone saves 15 %. Signatures are 84.5 % of the
weight, and only a signature decision moves that.

## 2. Bandwidth, storage, CPU per validator

Arithmetic from the measured sizes and rates, one copy of each transaction
received. Gossip duplication multiplies the bandwidth column; it was not
measured.

| TPS | Form | Ingest bandwidth | Storage per day (bodies) | Verify cores needed |
|---|---|---|---|---|
| 1,000 | hybrid 13,215 B | 106 Mbit/s | 1.14 TB | 0.92 (1,082 verify/s/core) |
| 10,000 | hybrid | 1.06 Gbit/s | 11.4 TB | 9.2 |
| 50,000 | hybrid | 5.29 Gbit/s | 57.1 TB | 46 |
| 1,000 | ML-DSA-65 + key hash 3,465 B | 28 Mbit/s | 0.30 TB | 0.18 (5,489/s/core) |
| 10,000 | ML-DSA-65 + key hash | 277 Mbit/s | 3.0 TB | 1.8 |
| 50,000 | ML-DSA-65 + key hash | 1.39 Gbit/s | 15.0 TB | 9.1 |

Verification rates, one core, optimized
(`cargo bench -p maya-crypto-pq --bench certificate`):

```
Ed25519 (classical)         20283 verify/s   pk    32 B   sig     64 B
ML-DSA-65                    5489 verify/s   pk  1952 B   sig   3309 B
ML-DSA-87                    3089 verify/s   pk  2592 B   sig   4627 B
SLH-DSA-SHA2-128s            1392 verify/s   pk    32 B   sig   7856 B
SLH-DSA-SHAKE-256f            125 verify/s   pk    64 B   sig  49856 B
Hybrid 65 + 128s             1082 verify/s   pk  1984 B   sig  11165 B
```

The node-level figure agrees: 1,010 verifications/s/core inside
`Transaction::verify` (`reports/12-baseline.md`).

**What the table says:** at 10,000 TPS with the hybrid, a validator needs a
10 Gbit/s link and 11 TB of disk a day for bodies. Signature pruning after
finality (§3) is the only lever on storage the brief names, and it is not
built.

## 3. Signature economy

- **FN-DSA (Falcon, FIPS 206):** not final as far as this session knows
  (`docs/CRYPTO_WATCH.md`, not checked live). It stays out of the registry
  and is labelled draft. Signing uses floating point, so wallets only.
- **Signature pruning with a STARK "all signatures verified" proof:** not
  built. It needs an ML-DSA and SLH-DSA verifier circuit in `crates/zk-stark`.
- **Vote certificates:** ADR-021. Option (a), a list plus bitmap, is measured
  at 216.5 KiB for n = 100, and n² per-round growth sets a 100-validator cap.
  Options (b) and (c) are not built, and the output says so.

## 4. Verification throughput

Parallel verification across cores is measured at 3.06x on 4 cores
(`apply_pipeline`). GPU verification: not built. The GPU HAL has mining
kernels only. The verified-signature cache is designed but not built
(`reports/12-performance.md` §1).

## 5. Propagation and erasure-coded dispersal

```
$ cargo bench -p maya-da --bench da_bench
== leader upload: plain gossip vs erasure-coded dispersal (MODEL, 1e9 bit/s uplink) ==
n=100 batch 1 MiB: gossip upload    8.0 MiB (   192 ms)   RS upload    3.0 MiB (   150 ms)   2.6x less
n=100 batch 8 MiB: gossip upload   64.0 MiB (   662 ms)   RS upload   24.2 MiB (   328 ms)   2.6x less
n=300 batch 1 MiB: gossip upload    8.0 MiB (   192 ms)   RS upload    3.0 MiB (   150 ms)   2.7x less
n=300 batch 8 MiB: gossip upload   64.0 MiB (   662 ms)   RS upload   24.0 MiB (   326 ms)   2.7x less
```

This is a **model**, and its output says so. It has no five-region latency
matrix, so it is not the simulation the brief asks for. Compact block relay
are only partly there: `crates/dag-bft` vertices reference transactions by
id (worker-batch digests in a node design, not built), but bytes on the wire per finalized transaction were not measured
end to end.

## 6. Data availability

```
== 2D Reed-Solomon extension (BLAKE3 Merkle commitments) ==
k= 16 block    128 KiB -> extended    512 KiB, encode     15.5 ms, header 2048 B, cell proof 160 B
k= 32 block   1024 KiB -> extended   4096 KiB, encode     15.1 ms, header 4096 B, cell proof 192 B
k= 64 block   8192 KiB -> extended  32768 KiB, encode    108.3 ms, header 8192 B, cell proof 224 B
```

Commitments are BLAKE3 Merkle trees. There is **no KZG path** anywhere in
`crates/da`. The withholding test (`crates/da/tests/da_tests.rs`):
an adversarial proposer withholds 30 % of chunks and sampling light clients
must detect it at the rate theory predicts.

```
$ cargo test -p maya-da --profile ci -- --nocapture
minimal withholding (28.2% of cells): detection 0.9950, theory 0.9950
test the_minimal_unrecoverable_withholding_is_unrecoverable_and_still_detected ... ok
30% withheld: 1994/2000 clients detected (0.9970); theory 0.9967 for 16 samples
test a_proposer_withholding_30_percent_is_caught_by_sampling_clients ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
```
 These live in `crates/da/tests`, not `sim/`, for the
dependency reason given in `reports/15-spec.md`.

## 7. Compression

zstd on the 400 real transactions (a 100-transaction test batch;
dictionary trained on the other 300):

| | Bytes | Saving |
|---|---|---|
| raw batch | 1,321,500 | — |
| `zstd -19` | 1,145,688 | 13.3 % |
| `zstd -19` + trained dictionary | 1,115,902 | 15.6 % |
| per transaction, `zstd -19`, no dictionary | 1,319,856 | 0.1 % |
| per transaction, with dictionary | 1,118,738 | 15.3 % |

**Signatures do not compress.** Almost all of the saving is the 16 senders'
public keys repeating, the bytes that keeping keys off the wire removes
anyway. On the "after" form, expect close to nothing.

## 8. Key registration and rotation

`crates/smart-account/tests/account_tests.rs`, 10 tests with real ML-DSA-65.
They include:

- `a_transfer_carries_a_key_hash_not_a_public_key` (registration once, a hash
  after);
- `rotation_keeps_the_address_rejects_the_old_key_and_accepts_the_new_one`;
- `a_replayed_op_is_refused`.

`cargo test -p maya-smart-account --profile ci`: `test result: ok. 10 passed; 0 failed`.
`crates/smart-account` is not wired into
the node's transaction format (ADR-016 names it core for launch).
