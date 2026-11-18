# Finality light client: checkpoint acceptance cost (ADR-089, M0)

Date: 2026-11-10. Commit: 54afb22b (the tree before the bench; the bench changes no code
beyond the uncommitted finality module, see "Tree state" below).

## What is measured

`FinalityClient::accept_checkpoint` on a checkpoint carrying exactly the quorum n − f:
header-id binding, trusted-period check, and the node's `Checkpoint::verify` (ML-DSA-87
signature check per signer). Quorum f = floor((n − 1) / 3).

| Committee n | Quorum | Low | Estimate | High |
|---|---|---|---|---|
| 7 | 5 | 7.8686 ms | 10.831 ms | 14.153 ms |
| 100 | 67 | 123.95 ms | 152.63 ms | 184.15 ms |

Criterion 0.7, 100 samples each. Outliers: n=7 21 of 100 (16 severe); n=100 5 of 100.
The n=100 run warned that 100 samples did not fit the 5 s target and took 10.3 s.
The spread is wide; treat these as one machine's point estimates, not a bound.

## Hardware, OS, command

- CPU: Intel Core Ultra 9 275HX, 24 cores / 24 logical processors
- RAM: 33,690,607,616 bytes
- OS: Microsoft Windows 11 Pro (build 10.0.29680)
- Profile: `bench` (optimized)
- Command: `cargo bench -p maya-light-client --bench finality`, run through
  `bash /d/Temp/claude-locks/cargo-slot.sh feat-light-client-finality`
- Output: `benches/finality.rs` run, raw output in the session scratchpad (bench-run.txt)

## Not measured

- Phone verification cost (research note phase 1). MISSING.
- Verification of a single ML-DSA-87 signature in isolation. Not separated from the
  client path here.
- Any network or multi-machine figure. This is one machine, localhost, no I/O.

## Tree state

The measured code is the uncommitted finality module on top of 54afb22b. The bench is
not a regression gate; `cargo xtask bench-gate` does not run for this crate.
