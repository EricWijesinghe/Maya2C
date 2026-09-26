# Runbook: Memory pressure / OOM kill

**Symptom.** Process restarted by the kernel; `dmesg` shows OOM.

**Check.** RSS over time; mempool size; RocksDB block cache setting.

**Act.** Lower the block cache in `node.toml`; cap mempool; add memory. An idle node is ~80 MiB (`docs/NODE_TYPES.md`).

**Rehearsal.** not rehearsed
