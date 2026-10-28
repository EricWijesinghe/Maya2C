# Audit findings tracker

Every finding gets a row. It is closed only when it has a fix commit, a
regression test, and a re-review by the firm that raised it. `cargo xtask
go-no-go` reads this table. Any row with severity `critical` or `high` that
is not `closed` is a NO-GO, and so is an empty table: no findings means no
audit, not a clean one.

| ID | Scope | Severity | Title | Issue | Fix commit | Regression test | Re-review | Status |
|---|---|---|---|---|---|---|---|---|

(No external audit has reported. Rows start at `F-001`.)

## Self-run pre-audit, 2026-10-29

**These are our own findings, not an audit.** They come from a security-agent
pass over every consensus diff merged since 2026-10-19, plus the open
re-genesis branches (#218, #221, #222, #230). Nothing independent reviewed
them. `cargo xtask go-no-go` counts an open `critical` or `high` row here as a
NO-GO, the same as an `F-` row. It never counts these rows as the external
report that the table above still lacks.

| ID | Scope | Severity | Title | Issue | Fix commit | Regression test | Re-review | Status |
|---|---|---|---|---|---|---|---|---|
| S-001 | state/staking.rs, staking/ops.rs | critical | Staking record decoder used the wire's 65,536 count cap; about 65,537 one-unit undelegations made every block fail to apply. Fixed: zero and dust undelegations refused, same-epoch unbonding entries merged (consensus-visible: changes state roots and slash rounding, so it ships only with the re-genesis) | fix/preaudit-staking | pending | `a_record_above_the_wire_count_cap_still_decodes`, `undelegations_cannot_grow_the_unbonding_queue_for_free` | self | open |
| S-002 | dag-bft/validator.rs | high | Any `Fetch` is answered with a full certificate (up to 4 MiB of batch), with no per-peer limit | fix/preaudit-staking | pending | `fetch_replies_are_budgeted_per_member` | self | open |
| S-003 | network/node/guard.rs, bus.rs | high | BFT frames are relayed after a version-byte check only; 512 × 5 MiB channel. Fixed: frames must decode before relay, and senders are scored. Still open: the channel is bounded by count, not bytes | fix/preaudit-staking | pending | `a_frame_that_does_not_decode_is_never_relayed` | self | open |
| S-004 | consensus/bft/auth.rs, vertex.rs | high | Vertex signatures carry no chain tag: an equivocation on one chain slashes a reused key on another | fix/preaudit-chaintag | pending | pending | self | open |
| S-005 | state/staking_exec.rs | medium | Evidence is refused once the committee record is pruned, though the offender is still bonded. Kept by design (an evidence age limit, as in Cosmos); the doc that promised otherwise is corrected | fix/preaudit-staking | pending | n/a | self | open |
| S-006 | state/undo.rs | medium | The undo journal refuses a previous value above 16 MiB, and the staking record is journalled every block. Fixed: a staking transaction that would grow the record past 8 MiB is refused, so the transaction fails, never the block. Shrinking transactions and evidence are never refused, so a full record cannot block a slash | fix/preaudit-staking | pending | pending | self | open |
| S-007 | fee-market/limits.rs | low | The fee floor of 1 base unit is 10⁶ cheaper at 24 decimals | fix/preaudit-chaintag | pending | pending | self | open |
| S-008 | fee-market/supply.rs | info | The 21M cap is a genesis input, not a protocol rule (ADR-046 decision 3) | fix/preaudit-chaintag | pending | pending | self | open |
| S-009 | consensus/bft/certsync.rs | low | A low head reported by a peer ends catch-up early (liveness only) | | | | self | open |
| S-010 | consensus/bft/certsync.rs | low | Per-page size is not capped on the client side; relies on the 10 MiB transport limit | | | | self | open |
