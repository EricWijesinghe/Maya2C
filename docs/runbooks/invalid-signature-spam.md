# Runbook: Invalid-signature spam

**Symptom.** Verification CPU saturated by transactions that fail TX-1.

**Check.** Rejections by peer.

**Act.** Disconnect peers whose cost exceeds contribution (dos-guard ledger); the mempool already rejects before admission.

**Rehearsal.** simulated: honest p99 32.5 s → 0.1 s with the guard, `attack_sim.rs`
