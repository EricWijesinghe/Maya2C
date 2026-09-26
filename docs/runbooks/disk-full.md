# Runbook: Disk full

**Symptom.** Writes fail; RocksDB returns `No space left on device`; the node stops importing blocks.

**Check.** `df -h` on the data volume; `du -sh <data-dir>/*`; is pruning on (`--prune-depth`)?

**Act.** Free space or grow the volume; enable pruning; restart. RocksDB writes are atomic per block (invariant 26), so a full disk cannot leave a half-committed block.

**Rehearsal.** not rehearsed; crash-atomicity that makes it safe is `crates/node/tests/crash_consistency_tests.rs` (1,000 SIGKILLs)
