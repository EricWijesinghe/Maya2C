# Runbook: Coordinated restart

**Symptom.** After a halt or a consensus bug, operators must restart from one agreed state.

**Check.** The candidate height and state root published by at least two independent operators.

**Act.** Each operator signs `(height, state_root)`; once ≥ ⅔ have signed, every node restarts and checks its own root at that height before producing.

**Rehearsal.** rehearsed: `crates/node/tests/coordinated_restart_rehearsal.rs`
