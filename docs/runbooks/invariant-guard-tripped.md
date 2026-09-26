# Runbook: Invariant guard tripped

**Symptom.** A module goes read-only for 100 blocks; transfers keep working.

**Check.** Which invariant; which block.

**Act.** Treat as a security incident: the guard stopped value creation. Do not override it; investigate the block that tripped it.

**Rehearsal.** rehearsed: `crates/node/tests/exploit_replays.rs` (breaker trips in the violating block)
