# Runbook: Node fell behind the tip

**Symptom.** `maya_block_observed_age_seconds` p99 rising; `maya_chain_height` flat while peers advance.

**Check.** `get_tip_height` against two other nodes; `maya_peers_connected`; CPU on verification.

**Act.** Check peers (see peer-starvation); if far behind, restart with `--bootstrap-from` a trusted node; verify the tip header against a second source.

**Rehearsal.** rehearsed: late nodes catch up after restart, `crates/node/tests/upgrade_rehearsal.rs`
