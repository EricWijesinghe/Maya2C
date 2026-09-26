# Runbook: Handshake flood

**Symptom.** CPU in the KEM path; many half-open connections.

**Check.** Handshake rate by subnet.

**Act.** Enable cookies and per-subnet budgets (dos-guard); raise puzzle difficulty.

**Rehearsal.** simulated: `crates/dos-guard/tests/attack_sim.rs` (not wired into the node)
