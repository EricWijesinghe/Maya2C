# Runbook: Stuck upgrade (upgrade required)

**Symptom.** Node refuses blocks with `upgrade required before height H`.

**Check.** Binary version vs the genesis `protocol_upgrades` schedule.

**Act.** Install a binary that supports the scheduled version and restart; it resumes from its last block. Do not edit genesis to skip the upgrade.

**Rehearsal.** rehearsed: 30 late nodes halt and catch up, `crates/node/tests/upgrade_rehearsal.rs`
