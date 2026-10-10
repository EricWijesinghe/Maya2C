# Attack Scenario Matrix

**Lab ID:** `maya2c-attacknet-lab-v1`
**Version:** 1.0
**Date:** 2026-10-09

## Scenario Categories & Coverage

| Cat | Name | Scenarios | Status | Priority | Automation |
|---|---|---|---|---|---|
| 1 | Consensus Adversary | 8 | 🔲 Planned | CRITICAL | `cargo xtask attacknet` (partial) |
| 2 | Network Adversary | 10 | 🔲 Planned | HIGH | `toxiproxy` + custom proxy |
| 3 | Transaction Adversary | 11 | 🔲 Planned | HIGH | Custom tx generator |
| 4 | API Adversary | 10 | 🔲 Planned | HIGH | `vegeta` + custom |
| 5 | VM/Contract Adversary | 9 | 🔲 Planned | MEDIUM | WASM fuzz targets |
| 6 | Storage Adversary | 10 | 🔲 Planned | HIGH | NTFS quota + file ops |
| 7 | Operational Adversary | 8 | 🔲 Planned | MEDIUM | Process/network control |
| 8 | Test-Key Compromise | 7 | 🔲 Planned | CRITICAL | Twin process spawn |

**Total: 73 scenarios**

---

## Category 1: Consensus Adversary

| ID | Scenario | Description | Expected | Implemented |
|---|---|---|---|---|
| 1.1 | `validator_silence` | One validator stops signing; others continue | No fork; silent validator catches up | ✅ attacknet |
| 1.2 | `delayed_proposals` | Validator proposes late (after anchor timeout) | Proposal ignored; no fork | 🔲 |
| 1.3 | `stale_proposals` | Validator proposes for old round | Rejected; no fork | 🔲 |
| 1.4 | `duplicate_votes` | Validator votes twice same round | Equivocation detected; slashed | 🔲 |
| 1.5 | `conflicting_votes` | Validator votes for two different blocks | Equivocation detected; slashed | 🔲 |
| 1.6 | `equivocation_evidence` | Honest validators collect equivocation proof | Evidence gossiped; tombstone created | 🔲 |
| 1.7 | `validator_restart_loops` | Validator crashes/restarts rapidly | Others unaffected; no fork | 🔲 |
| 1.8 | `quorum_edge_conditions` | Exactly f/f+1 validators down | f: continues; f+1: halts safely | ✅ attacknet |

### Partition Variants (1.9–1.13)

| ID | Scenario | Description | Expected |
|---|---|---|---|
| 1.9 | `minority_partition` | 1 validator isolated from 3 | Isolated halts; 3 continue |
| 1.10 | `majority_partition` | 3 validators isolated from 1 | Isolated 3 continue; 1 halts |
| 1.11 | `symmetric_partition` | 2+2 split | Both halt (no quorum) |
| 1.12 | `asymmetric_partition` | A→B ok, B→A dropped | Directional degradation |
| 1.13 | `partition_healing` | Partition resolves after 30s | Single chain resumes |
| 1.14 | `stale_node_rejoin` | Validator down > GC window | Catch-up via checkpoints |
| 1.15 | `validator_set_transition_during_faults` | Epoch change during partition | Clean transition or safe halt |

---

## Category 2: Network Adversary

| ID | Scenario | Description | Tool | Implemented |
|---|---|---|---|---|
| 2.1 | `malformed_frames` | Invalid libp2p frame encoding | Custom proxy | 🔲 |
| 2.2 | `invalid_handshakes` | Noise XX handshake with wrong keys | Custom proxy | 🔲 |
| 2.3 | `random_bytes` | Raw TCP garbage on P2P ports | `attacknet` garbage | ✅ |
| 2.4 | `oversized_messages` | Frames > max size (4MB) | Custom proxy | 🔲 |
| 2.5 | `incomplete_messages` | Truncated frames mid-stream | Custom proxy | 🔲 |
| 2.6 | `slow_transmissions` | 1 byte/sec send rate | `toxiproxy` latency | 🔲 |
| 2.7 | `rapid_connection_churn` | Connect/disconnect 100/sec | Custom script | 🔲 |
| 2.8 | `peer_identity_churn` | New peer ID every connection | Custom proxy | 🔲 |
| 2.9 | `invalid_peer_announcements` | Fake peer addresses in gossip | Custom proxy | 🔲 |
| 2.10 | `eclipse_topology` | Surround victim with adversary | Network namespaces | 🔲 |
| 2.11 | `delayed_traffic` | 500ms latency injection | `tc` / `toxiproxy` | 🔲 |
| 2.12 | `dropped_traffic` | 10% packet loss | `tc` / `toxiproxy` | 🔲 |
| 2.13 | `duplicated_traffic` | 5% packet duplication | `tc` / `toxiproxy` | 🔲 |
| 2.14 | `reordered_traffic` | 25% reorder | `tc` / `toxiproxy` | 🔲 |
| 2.15 | `bandwidth_constraints` | 1 Mbps cap | `tc` / `toxiproxy` | 🔲 |
| 2.16 | `connection_resets` | RST after handshake | `toxiproxy` reset | 🔲 |
| 2.17 | `dns_failure` | Bootnode DNS resolution fails | hosts file | 🔲 |
| 2.18 | `routing_failure` | ICMP unreachable | `nftables` | 🔲 |

---

## Category 3: Transaction Adversary

| ID | Scenario | Description | Tool |
|---|---|---|---|
| 3.1 | `invalid_signatures` | Wrong curve/params | Custom tx gen |
| 3.2 | `corrupted_signatures` | Bit-flipped signature bytes | Custom tx gen |
| 3.3 | `wrong_scheme` | Ed25519 sig for ML-DSA key | Custom tx gen |
| 3.4 | `wrong_params` | ML-DSA-65 sig for ML-DSA-87 | Custom tx gen |
| 3.5 | `domain_separation` | Sig from wrong chain/domain | Custom tx gen |
| 3.6 | `wrong_chain_id` | Mainnet tx on lab chain | Custom tx gen |
| 3.7 | `replayed_transactions` | Valid tx submitted twice | Custom tx gen |
| 3.8 | `duplicate_transactions` | Same nonce, different sig | Custom tx gen |
| 3.9 | `invalid_nonces` | Gap, overflow, reuse | Custom tx gen |
| 3.10 | `balance_underflow` | Spend > balance | Custom tx gen |
| 3.11 | `arithmetic_boundaries` | u64::MAX, 0, overflow | Custom tx gen |
| 3.12 | `max_size_transactions` | 1MB+ tx payloads | Custom tx gen |
| 3.13 | `malformed_encoding` | Invalid SCALE/JSON | Custom tx gen |
| 3.14 | `unknown_variants` | Future tx types | Custom tx gen |
| 3.15 | `fee_edge_cases` | 0 fee, max fee, dust | Custom tx gen |
| 3.16 | `high_volume_invalid` | 1000 invalid tx/sec | Load generator |

---

## Category 4: API Adversary

| ID | Scenario | Description | Tool |
|---|---|---|---|
| 4.1 | `malformed_json` | Truncated, extra commas, wrong types | `vegeta` custom |
| 4.2 | `malformed_rpc` | Invalid JSON-RPC 2.0 | `vegeta` custom |
| 4.3 | `oversized_bodies` | 10MB+ request bodies | `vegeta` custom |
| 4.4 | `excessive_nesting` | 1000-level JSON nesting | `vegeta` custom |
| 4.5 | `invalid_encodings` | UTF-16, binary in JSON | `vegeta` custom |
| 4.6 | `unsupported_methods` | `method_not_found` flood | `vegeta` custom |
| 4.7 | `high_concurrency` | 1000 parallel connections | `vegeta` / `wrk` |
| 4.8 | `slow_clients` | 1 byte/sec request send | `toxiproxy` |
| 4.9 | `abandoned_connections` | Connect, send partial, close | Custom |
| 4.10 | `websocket_churn` | Connect/disconnect WS rapidly | Custom |
| 4.11 | `subscription_floods` | 1000 subscriptions/sec | Custom |
| 4.12 | `pagination_abuse` | page=999999, limit=MAX | Custom |
| 4.13 | `expensive_queries` | Deep state scans | Custom |
| 4.14 | `rate_limit_verification` | Exceed 1000/s, verify 429 | `vegeta` |
| 4.15 | `auth_tests` | Invalid tokens, expired JWT | Custom |
| 4.16 | `error_leakage` | Check error msgs for secrets | Custom |

---

## Category 5: VM/Contract Adversary

| ID | Scenario | Description | Tool |
|---|---|---|---|
| 5.1 | `infinite_loops` | WASM `loop {}` | Fuzz target |
| 5.2 | `computational_exhaustion` | 10^9 iterations | Fuzz target |
| 5.3 | `memory_exhaustion` | Allocate 4GB in WASM | Fuzz target |
| 5.4 | `storage_exhaustion` | Write 10^6 keys | Fuzz target |
| 5.5 | `max_call_depth` | 1000 nested calls | Fuzz target |
| 5.6 | `malformed_bytecode` | Invalid WASM sections | Fuzz target |
| 5.7 | `invalid_imports` | Missing host functions | Fuzz target |
| 5.8 | `forbidden_host_calls` | Access denied host fns | Fuzz target |
| 5.9 | `deterministic_execution` | Same input → different output | Differential |
| 5.10 | `metering_boundaries` | Fuel exactly at limit | Fuzz target |
| 5.11 | `contract_panic` | `unreachable` in WASM | Fuzz target |
| 5.12 | `repeated_reverts` | 100 reverts in block | Fuzz target |
| 5.13 | `adversarial_state_growth` | Maximize state per tx | Fuzz target |

---

## Category 6: Storage Adversary

| ID | Scenario | Description | Tool |
|---|---|---|---|
| 6.1 | `abrupt_termination` | Kill -9 during write | `Stop-Process -Force` |
| 6.2 | `partial_file_replacement` | Swap SST files mid-run | PowerShell |
| 6.3 | `corrupted_database` | Bit-flip in .sst | Custom corruptor |
| 6.4 | `corrupted_snapshot` | Truncated snapshot | PowerShell |
| 6.5 | `truncated_snapshots` | Snapshot at 50% | PowerShell |
| 6.6 | `stale_snapshots` | 1000-block-old snapshot | PowerShell |
| 6.7 | `wrong_network_snapshot` | Mainnet snapshot on lab | PowerShell |
| 6.8 | `full_disk_simulation` | NTFS quota at 99% | `fsutil quota` |
| 6.9 | `read_only_storage` | ACL deny write | `icacls` |
| 6.10 | `missing_files` | Delete MANIFEST | PowerShell |
| 6.11 | `incorrect_permissions` | ACL deny read | `icacls` |
| 6.12 | `restore_verified_backup` | Snapshot restore test | PowerShell |
| 6.13 | `detect_unverified_backup` | Reject tampered snapshot | PowerShell |

---

## Category 7: Operational Adversary

| ID | Scenario | Description | Tool |
|---|---|---|---|
| 7.1 | `expired_certificates` | TLS cert expired | Custom cert |
| 7.2 | `broken_dns` | Resolver returns NXDOMAIN | hosts / dnsmasq |
| 7.3 | `bootnode_unavailable` | Bootnode process killed | `Stop-Process` |
| 7.4 | `gateway_unavailable` | RPC gateway killed | `Stop-Process` |
| 7.5 | `monitoring_unavailable` | Prometheus down | `docker stop` |
| 7.6 | `stale_configuration` | Config from old version | Config swap |
| 7.7 | `incompatible_binary` | Old binary with new genesis | Binary swap |
| 7.8 | `failed_rolling_upgrade` | 2/4 upgraded, 2 old | Binary swap |
| 7.9 | `rollback` | Upgrade → downgrade | Binary swap |
| 7.10 | `restart_after_host_shutdown` | Cold boot all | Host reboot |

---

## Category 8: Test-Key Compromise Drills

| ID | Scenario | Description | Tool |
|---|---|---|---|
| 8.1 | `copied_validator_key` | Twin process with same key | `Run-Attacks.ps1` |
| 8.2 | `double_sign_detection` | Verify equivocation logged | Log analysis |
| 8.3 | `evidence_handling` | Equivocation → tombstone | Log analysis |
| 8.4 | `slashing_behavior` | Stake reduced, validator jailed | State check |
| 8.5 | `validator_removal` | Tombstoned validator excluded | Committee check |
| 8.6 | `key_rotation` | Replace compromised key | Manual (future) |
| 8.7 | `incident_communication` | Alert template fired | Alertmanager |

---

## Execution Schedule (4-Week Rotation)

| Week | Days | Categories | Focus |
|---|---|---|---|
| 1 | Mon-Fri | 1, 2 | Consensus + Network safety |
| 1 | Sat-Sun | 3, 4 | Transaction + API robustness |
| 2 | Mon-Fri | 5, 6 | VM + Storage integrity |
| 2 | Sat-Sun | 7, 8 | Operations + Key compromise |
| 3 | Mon-Fri | 1, 3, 5 | Combined: consensus under tx pressure |
| 3 | Sat-Sun | 2, 4, 6 | Combined: network + API + storage |
| 4 | Mon-Fri | All | Full combined stress (Cat F) |
| 4 | Sat-Sun | 8 | Key compromise deep dive |

## Scenario Selection Algorithm (Daily)

```python
def select_daily_scenarios(day: int, week: int) -> List[Scenario]:
    # Always run: 1.1, 1.8 (baseline consensus)
    # Rotate through category scenarios
    # Weight by: severity, recent failures, coverage gaps
    pass
```

## Tracking

- **Implemented:** ✅ in `cargo xtask attacknet` or script
- **Planned:** 🔲 documented, not yet automated
- **In Progress:** 🟡 partially implemented
- **Blocked:** 🔴 dependency missing

Update this matrix as scenarios are implemented.