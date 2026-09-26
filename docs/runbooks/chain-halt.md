# Runbook: Chain halt (liveness failure)

**Symptom.** `ProductionStalled` fires: no block imported for 150 s.

**Check.** Is it this node (see fell-behind) or everyone? Check two independent explorers or operators.

**Act.** If network-wide: coordinated restart from an agreed height and state root (docs/runbooks/coordinated-restart.md).

**Rehearsal.** rehearsed: `crates/node/tests/coordinated_restart_rehearsal.rs`
