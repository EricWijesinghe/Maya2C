# Apply time per MiB at 1, 2.5 and 5 MiB blocks (ADR-072): NOT MEASURED

Status: blocked. No figure in this file is a measurement.

- Commit: c219bd5c (`origin/fix/max-block-bytes`) plus the `fix/subdag-decode-bound` changes.
  That commit is not built, so the figures are not for it.
- Host: this Windows 11 Pro 10.0.29680 machine, 32.9 GiB RAM (launch host).
- Blocker: the release bench (`crates/node/benches/apply_pipeline.rs`) needs a release build of
  `custom-l1-node`. The debug lib test build at `CARGO_BUILD_JOBS=1` had reached 69 of roughly
  1,167 lockfile packages after about 7 hours (`D:/Temp/claude-dag-warm.log`). No release build was
  started.

## Command to run once the build exists (not run)

```
CARGO_BUILD_JOBS=1 cargo bench -p custom-l1-node --bench apply_pipeline 2>&1 | tail -40
```

Read the existing bench docs for the block sizes (1, 2.5, 5 MiB of standard hybrid transfers) and
for the release profile before running. Record: median and p95 apply time per block, commit sha,
CPU model, RAM, OS build, exact command.

## Fields to fill

| Block | Median apply (ms) | p95 apply (ms) | ms per MiB |
|---|---|---|---|
| 1 MiB | MISSING | MISSING | MISSING |
| 2.5 MiB | MISSING | MISSING | MISSING |
| 5 MiB | MISSING | MISSING | MISSING |

CPU: MISSING. RAM: MISSING. OS build: MISSING (Windows 11 Pro 10.0.29680 is the host, not measured
against). Block interval: `reports/bench/2026-10-25-tps-series.md` measured about 2.6 s for a
400-transfer block on a shared machine; that is the only interval figure on record.

## Conclusion

5 MiB is not confirmed. ADR-072 derived about 0.2 to 0.5 s per block from 2026-09-26 figures, not
from a measurement. No recommendation to change `MAX_BLOCK_BYTES` follows from this file.
