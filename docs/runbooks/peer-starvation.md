# Runbook: Peer starvation

**Symptom.** `maya_peers_connected` below 4; blocks arrive late or not at all.

**Check.** Firewall on the p2p port (30333); `--bootnode` addresses reachable; inbound limits (per-subnet caps).

**Act.** Add bootnodes from independent operators; open the port; if many peers share one subnet, suspect an eclipse (see eclipse-suspected).

**Rehearsal.** not rehearsed on a network; diversity rules simulated in `crates/dos-guard/tests/attack_sim.rs`
