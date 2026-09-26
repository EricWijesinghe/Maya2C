# Runbooks

The top 20 failure modes (Master Prompt 19 §2). Each says how it was rehearsed, or that it was not.

| Runbook | Rehearsed |
|---|---|
| [Disk full](disk-full.md) | not rehearsed; crash-atomicity that makes it safe is `crates/node/tests/crash_consistency_tests.rs` (1,000 SIGKILLs) |
| [Node fell behind the tip](fell-behind.md) | rehearsed |
| [Peer starvation](peer-starvation.md) | not rehearsed on a network; diversity rules simulated in `crates/dos-guard/tests/attack_sim.rs` |
| [Remote signer unreachable](signer-unreachable.md) | rehearsed in tests |
| [Clock drift](clock-drift.md) | not rehearsed; Marzullo fusion in `crates/timing` is tested |
| [Stuck upgrade (upgrade required)](stuck-upgrade.md) | rehearsed |
| [RPC overload](rpc-overload.md) | measured |
| [Snapshot corrupt or lying peer](snapshot-corrupt.md) | rehearsed |
| [Chain halt (liveness failure)](chain-halt.md) | rehearsed |
| [Coordinated restart](coordinated-restart.md) | rehearsed |
| [State root mismatch](state-root-mismatch.md) | pinned by invariant 24 tests; not rehearsed as an incident |
| [Database corruption](db-corruption.md) | not rehearsed; process-crash safety is `crash_consistency_tests.rs` |
| [Memory pressure / OOM kill](memory-pressure.md) | not rehearsed |
| [Handshake flood](handshake-flood.md) | simulated |
| [Invalid-signature spam](invalid-signature-spam.md) | simulated |
| [Eclipse suspected](eclipse-suspected.md) | simulated |
| [Mempool full](mempool-full.md) | not rehearsed |
| [Deep reorganisation](deep-reorg.md) | pinned |
| [Validator or operator key compromise](key-compromise.md) | not rehearsed |
| [Invariant guard tripped](invariant-guard-tripped.md) | rehearsed |
