# Runbook: Snapshot corrupt or lying peer

**Symptom.** State sync stalls or bans peers.

**Check.** Which peers were banned; manifest root vs the on-chain root.

**Act.** Nothing to fix on the joiner: bad chunks are rejected and the sender banned. If every peer is banned, obtain a manifest root from an independent operator.

**Rehearsal.** rehearsed: `crates/state-sync/tests/sync_tests.rs`, 100M-account sim with 4 liars (`reports/14-scale.md`)
