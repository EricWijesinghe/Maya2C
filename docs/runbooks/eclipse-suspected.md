# Runbook: Eclipse suspected

**Symptom.** Most peers share few subnets; the tip lags public explorers.

**Check.** Peer list grouped by /24 and /16.

**Act.** Disconnect and re-dial from a curated, diverse bootnode list; cap per-subnet inbound.

**Rehearsal.** simulated: attacker with 70% of attempts holds 9/50, `attack_sim.rs`
