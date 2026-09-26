# ADR-021: Vote certificates — a signature list for v1, a validator cap from its measured cost

**Status:** Accepted
**Date:** 2026-09-26

## Context

Master Prompt 13 §3: a quorum certificate of 2f+1 ML-DSA signatures is
large. Choose between (a) a plain list with a signer bitmap, (b) a STARK proving
2f+1 valid signatures, and (c) hash-based multisignatures (RESEARCH). The
choice must rest on measured size and proving time, and proving must fit the
block interval.

## Measurements

`cargo bench -p maya-crypto-pq --bench certificate`, optimized, 4 cores
(hardware in `reports/12-baseline.md`):

```
ML-DSA-65                    5489 verify/s   pk  1952 B   sig   3309 B
ML-DSA-87                    3089 verify/s   pk  2592 B   sig   4627 B
MlDsa65                n=100 quorum= 67    221716 B ( 216.5 KiB)  verify serial   12.28 ms  4 cores    7.95 ms
MlDsa65                n=200 quorum=133    440122 B ( 429.8 KiB)  verify serial   24.52 ms  4 cores   10.61 ms
MlDsa65                n=400 quorum=267    883553 B ( 862.8 KiB)  verify serial   48.62 ms  4 cores   21.10 ms
option (b) STARK-aggregated certificate: NOT BUILT (needs an ML-DSA verifier circuit in zk-stark)
option (c) hash-based multisignature: NOT BUILT (RESEARCH; no standardised scheme)
```

One certificate is cheap: under 25 ms to verify on 4 cores at any size
measured. The cost is in how many there are. A Narwhal-style DAG carries a
certificate for every vertex, n per round, so per-round load grows as n².
From the measured sizes and rates (arithmetic, not a network measurement):

| n | cert | certs per round | at 1 s rounds | at 2 s rounds |
|---|---|---|---|---|
| 50 | 106.6 KiB | 5.2 MiB | 44 Mbit/s, 0.30 cores | 22 Mbit/s, 0.15 cores |
| 100 | 216.5 KiB | 21.1 MiB | 177 Mbit/s, 1.22 cores | 89 Mbit/s, 0.61 cores |
| 200 | 429.8 KiB | 83.9 MiB | 704 Mbit/s, 4.85 cores | 352 Mbit/s, 2.42 cores |
| 400 | 862.8 KiB | 337.0 MiB | 2,827 Mbit/s, 19.5 cores | 1,414 Mbit/s, 9.7 cores |

## Decision

1. **v1 uses option (a) with ML-DSA-65.** It is the only option that exists and
   was measured. (b) and (c) cannot be chosen by measurement because neither
   is built. ML-DSA-87 is 40 % larger and 44 % slower to verify for a security
   category the rest of the chain does not use.
2. **The validator set is capped at 100 for v1.** The finality SLO (p50 ≤ 2 s,
   `docs/SLO.md`) needs rounds of about 1 s under Bullshark's two-round
   commit. At that pace, n = 100 costs 177 Mbit/s and 1.2 cores for
   certificates alone, inside a 1 Gbit/s, 4-core validator with headroom for
   transactions. n = 200 would take 70 % of the NIC and more than the cores.
   This is the "current design limit" Master Prompt 14 §6 asks for, derived
   from measured costs. It has not been observed in a geographic simulation.
3. **Option (b) is the planned path past 100**, and its acceptance test is
   fixed now. The proof for one certificate must be generated within one round
   on validator-class hardware, and it must verify in less time than the
   n = 100 list takes (7.95 ms on 4 cores).

## Consequences

- `crates/dag-bft` does not yet carry real signatures on its certificates;
  the simulation uses abstract votes. Wiring (a) in is part of wiring DAG-BFT
  into the node (ADR-015).
- The cap is a genesis-level parameter, not a constant. Raising it is a MIP
  with a new measurement.
