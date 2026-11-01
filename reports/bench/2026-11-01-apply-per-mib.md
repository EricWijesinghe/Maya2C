# Apply time per MiB at the ADR-072 cap and the CON-13 decode budget

Measured 2026-11-01.

- Commit: `cbf6957b` (master plus #278 `fix/subdag-decode-bound`), with the one-line bench fix in this PR. Without the fix the bench panicked: `Storage("StateDB not bound to a chain")`. Apply now refuses an unbound StateDB, and the bench had not been updated for it.
- Host: Intel(R) Core(TM) Ultra 9 275HX, 24 logical cores, 32,130 MB RAM, Windows 11 Pro Insider 10.0.29680, NitroZeus. rustc 1.99.0-nightly (da80ed070 2026-07-14).
- Profile: release (`cargo bench`).
- Load: CARGO_BUILD_JOBS=2; a WSL Kani run was going in parallel, so these figures are an upper bound.
- Command: `CARGO_BUILD_JOBS=2 cargo bench -p custom-l1-node --bench apply_pipeline`

## Raw output

```
apply_pipeline: 400 hybrid transfers, 100/block, 24 cores, optimized
verify   1 thread :    0.370 ms/tx      2700 tx/s  2 allocs/tx
verify 24 threads:    0.026 ms/tx     38789 tx/s  (x14.37)
apply    1 thread :    0.588 ms/tx      1700 tx/s  14 allocs/tx (includes verify)
  state work alone:    0.218 ms/tx  (37.0% of apply; 12 allocs/tx)
write amplification: NOT MEASURED (/proc/self/io unavailable on this platform)
peak RSS: NOT MEASURED (/proc/self/status unavailable)
```

## Per MiB (computed from the lines above)

A standard hybrid transfer is 11,165 bytes (ADR-072). So 1 MiB holds 93.9 transfers, and the 7.5 MiB decode budget (3/2 × the 5 MiB cap) holds 704.

| Path | ms/tx | ms per MiB | 5 MiB cap | 7.5 MiB budget |
|---|---|---|---|---|
| Serial apply, including serial verify | 0.588 | 55.2 | 276 ms | 414 ms |
| Parallel verify (24 threads), then serial state work | 0.026 + 0.218 | 22.9 | 115 ms | 172 ms |

The worst case, a full decode budget applied single-threaded, is 414 ms. That fits inside one block interval, so the provisional 5 MiB cap (ADR-072) stands on this host.

The run has 400 transfers in 4 blocks against a fresh DB. It does not measure a multi-GB state where RocksDB reads miss cache. That figure is MISSING and goes to BACKLOG.
