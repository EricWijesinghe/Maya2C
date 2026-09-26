# Runbook: Database corruption

**Symptom.** RocksDB refuses to open, or a root no longer matches its header.

**Check.** `maya2c-node` start-up errors; which column family.

**Act.** Stop, move the data dir aside, resync from a snapshot. Never hand-edit the store.

**Rehearsal.** not rehearsed; process-crash safety is `crash_consistency_tests.rs`
