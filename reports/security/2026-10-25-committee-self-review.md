# Committee self-review (PRE-AUDIT, self-run, 2026-10-25)

Read-only code review by a Claude subagent of `crates/staking`, the epoch and
checkpoint paths and `bins/maya2c-node/src/bft.rs`. **Nothing was executed:
the replay tests below are proposals, and no finding has been reproduced yet.**
This is not an audit and not independent. Each finding needs its replay test
to fail before a fix and pass after (Standing Order 4; rust-reviewer and
security-reviewer before any consensus change; Eric reviews).

| # | Sev | Finding | Where | Replay test (proposed) |
|---|---|---|---|---|
| 1 | High | A double-signer escapes slashing by unbonding everything before the boundary: the zero-stake record is removed, `slash()` returns `UnknownValidator`, unbonding entries release unslashed, id not retired so it can re-register | staking `epoch.rs:61-62`, `ops.rs:193-239`, `epoch.rs:182-193` | `fully_unbonded_offender_is_still_slashed` |
| 2 | High | Double-sign slashing has no production trigger: evidence is only logged | `bins/maya2c-node/src/bft.rs:605-610`; `ReportEquivocation` built only in tests | `equivocation_is_slashed_without_operator_action` |
| 3 | Critical at testnet shape | The 71.4% holder alone meets quorum (W-f = 933,333,334 < 1e9) and can sign a checkpoint alone; f <= W/3 is violated by one key. Design/config (ADR-040/041), Eric's decision | `dag-bft vertex.rs:118-131`, `attest.rs:204`, `vertex.rs:269` | `single_heavy_member_forms_checkpoint_quorum` |
| 4 | High | If the holder is offline there is no quorum and epoch boundaries (which run in blocks) cannot rotate it: no in-protocol recovery, only re-genesis if its key is lost | `staking_exec.rs:270-272,298`, `genesis.rs:931` | `whale_offline_halts_and_no_boundary_rotates_it` |
| 5 | High at this shape | A tombstoned equivocator keeps voting weight until the next boundary | `staking_exec.rs:263-264,317-325`, `ops.rs:197-205` | `tombstoned_member_still_counts_until_boundary` |
| 6 | Medium | Reward sniping: delegate in the boundary block, take a full pro-rata share | `db.rs:897-926`, `epoch.rs:109-119` | `boundary_block_delegation_earns_full_epoch_share` |
| 7 | Medium | Late evidence refused for still-bonded stake once the committee is dropped | `staking_exec.rs:226-232,328-336`, `ops.rs:191` | `late_evidence_for_bonded_offender_is_refused` |
| 8 | Medium | Empty-committee fallback restores `previous`, which can hold jailed members; zero-stake members get weight 1 | `staking_exec.rs:297-299`, `vertex.rs:43` | `all_jailed_keeps_previous_committee_with_jailed_members` |
| 9 | Medium | Re-registration of a removed id; `slash()` also cuts a new validator's old unbonding entries | `ops.rs:33,233-239` | follows from 1 |
| 10 | Low-Med | `Params::is_valid` misses `unbonding_epochs >= 1`, `jail_epochs >= 1`, `epoch_blocks >= 1` (0 stops epochs) | `params.rs:53-60`, `staking_exec.rs:271` | param-rejection tests |
| 11-15 | Low | dust-delegation growth; empty committee never judged downtime; header seal participation not cross-checked (SUSPECTED); seal epoch wraps at 2^24; weight shift above 2^64 | various | see source |

Checked and fine: staking crate is deterministic (BTree*, integers only);
downtime/jail arithmetic and the 2-epoch jail; quorum overlap for W >= 3f+1.

Not reviewed: `Amount::mul_div`, possession proof, `apply_effects` rollback,
`Equivocation::is_valid`, catch-up internals, governance parameter path, the
DAG engine, the ADR text against the code, testnet `min_delegation` and
`epoch_blocks` values.
