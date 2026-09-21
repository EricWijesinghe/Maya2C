# ADR-010: Entropy — every source health-tested, all mixed into one HMAC-DRBG

**Status:** Accepted
**Date:** 2026-09-21

## Context

Key material in the tree comes straight from `getrandom`. The brief asks for a
hardware abstraction (`hal/entropy`) with real sources (OS, RDSEED), simulated
physical sources (thermal, micro-voltage, Brownian motion in a microfluidic
channel, a vacuum-fluctuation QRNG), a Casimir-cavity source kept as research,
SP 800-90B health tests, an SP 800-90A DRBG, and tamper-driven zeroization.

## Decision

1. **`EntropySource` trait** returning raw samples plus a declared class
   (`Real` / `Sim` / `Research`) and a declared min-entropy per byte. A SIM
   source's `name()` starts with `sim-` and it logs that on construction.
2. **Health tests on every source, on every read**: the SP 800-90B §4.4.1
   repetition count test and §4.4.2 adaptive proportion test (window 512),
   with cutoffs computed from the declared min-entropy at α = 2^-20. A source
   that fails is disabled permanently for the life of the pool and reported;
   it is never silently re-enabled.
3. **One DRBG.** HMAC-DRBG with HMAC-SHA-256 (SP 800-90A Rev. 1 §10.1.2),
   validated against the NIST CAVP `HMAC_DRBG.rsp` vectors. It is seeded and
   reseeded from the concatenated output of *all* healthy sources, never one,
   and refuses to produce output if the healthy sources together claim less
   than 256 bits of min-entropy. The OS source is always one of them.
4. **Sim sources are models.** The QRNG simulates balanced homodyne detection
   of the vacuum quadrature: Gaussian samples digitised by an ADC, with
   electronic noise and a configurable squeezing/clearance ratio, which is
   what real devices measure. The model's *randomness* comes from the OS
   source; the model shapes it. It adds no entropy and says so in its docs.
5. **No SIM or RESEARCH source reaches a production pool without the
   `sim-sources` feature**, and a production build of the pool refuses to
   admit a source whose class is not `Real` unless that feature is on.
6. **RDSEED** is read through `core::arch::x86_64::_rdseed64_step` after a
   CPUID check, with a `// SAFETY:` comment and a test. `hal/` is outside the
   core execution crates, so this is within the execution directive on
   `unsafe`.
7. **Tamper.** `TamperLine::trip()` zeroizes every registered key store and
   the DRBG state, and latches; Master Prompt 8's firmware guard calls it.

## Alternatives considered

- **`rand_chacha` as the DRBG.** Not an SP 800-90A mechanism; the brief names
  the standard.
- **`hmac-drbg` crate.** Unmaintained and on `hmac 0.8`; 150 lines against
  `hmac 0.12` with the CAVP vectors is smaller than the audit of a dead crate.
- **CTR_DRBG.** Would need an AES dependency the tree otherwise lacks.

## Consequences

- The simulated sources are useful for testing the health tests and the
  pool's failure handling, and for nothing else.
- SP 800-22 and Dieharder are run offline by `scripts/entropy_battery.sh`
  (WSL) and their raw output saved to `reports/entropy/`; the tests in the
  crate never claim those batteries passed.
