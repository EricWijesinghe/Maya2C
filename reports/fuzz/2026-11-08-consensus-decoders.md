# Fuzz campaign: 8 consensus decoders (2026-11-08)

- commit: efbdd9ca (#374), checkout in WSL Ubuntu-24.04 at `/root/fuzzrun/Maya2C`
- engine: cargo-fuzz / libFuzzer, one target per process, RSS limit 1536 MB (as printed by the OOM line below)
- logs: `/root/fuzzrun/run-<target>.log` (WSL); final libFuzzer status line copied below

| target | execs | cov | corpus | result |
|---|---:|---:|---|---|
| account_decode | 56,936,922 | 38 | 2 / 41 b | DONE, 0 errors |
| block_decode | 54,330,018 | 1845 | 376 / 125 Kb | DONE, 0 errors |
| block_sync_response | 54,215,435 | 537 | 107 / 25 Kb | DONE, 0 errors |
| car_decode | 122,301,826 | 837 | 1173 / 72 Kb | DONE, 0 errors |
| header_decode | 57,517,014 | 78 | 11 / 1093 b | DONE, 0 errors |
| payload_decode | 55,420,033 | 2386 | 551 / 541 Kb | DONE, 0 errors |
| stateless_witness_decode (re-run, `run-sw2.log`) | 11,395,588 | 372 | 197 / 75 Kb | DONE, 0 errors |
| tx_decode | 50,145,069 | 2487 | 547 / 2818 Kb | DONE, 0 errors |
| **total** | **462,261,905** | | | **0 crashes** |

## The one non-clean run

The first `stateless_witness_decode` run (`run-stateless_witness_decode.log`)
stopped at 11,732,683 execs with `libFuzzer: out-of-memory (used: 1538Mb;
limit: 1536Mb)`. Its RSS was already about 1.5 GB at steady state, above the
other targets' 600 MB, so the limit was hit by the harness's baseline, not by
an allocation proportional to one input. The re-run (`run-sw2.log`, same
limit) finished 1,801 s and 11.4M execs with no error. Not counted in the
total. NOT VERIFIED: that no single input drives a large allocation in this
decoder; a run with a raised limit and `-malloc_limit_mb` would settle it.

One operator, one machine. A floor, not a substitute for continuous fuzzing
(OSS-Fuzz project definition: #384).
