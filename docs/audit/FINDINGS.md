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
| S-001 | state/staking.rs, staking/ops.rs | critical | Staking record decoder used the wire's 65,536 count cap; about 65,537 one-unit undelegations made every block fail to apply. Fixed: zero and dust undelegations refused, same-epoch unbonding entries merged (consensus-visible: changes state roots and slash rounding, so it ships only with the re-genesis) | fix/preaudit-staking | e342d670 | `a_record_above_the_wire_count_cap_still_decodes`, `undelegations_cannot_grow_the_unbonding_queue_for_free` | self | fixed |
| S-002 | dag-bft/validator.rs | high | Any `Fetch` is answered with a full certificate (up to 4 MiB of batch), with no per-peer limit | fix/preaudit-staking | e342d670 | `fetch_replies_are_budgeted_per_member` | self | fixed |
| S-003 | network/node/guard.rs, bus.rs | high | BFT frames are relayed after a version-byte check only; 512 × 5 MiB channel. Fixed: frames must decode before relay, and senders are scored. The channel now also holds at most 256 MiB of frames; one past that is dropped, not relayed, and re-fetched by the engine | fix/preaudit-staking | e342d670, fix/s003-frame-bytes | `a_frame_that_does_not_decode_is_never_relayed`, `large_frames_cannot_hold_more_than_the_byte_budget` | self | fixed |
| S-004 | consensus/bft/auth.rs, vertex.rs | high | Vertex signatures carry no chain tag: an equivocation on one chain slashes a reused key on another | fix/preaudit-chaintag | 299acd9d (#236) | `a_vote_signed_for_one_chain_does_not_verify_on_another` | self (not independent) | fixed |
| S-005 | state/staking_exec.rs | medium | Evidence is refused once the committee record is pruned, though the offender is still bonded. Kept by design (an evidence age limit, as in Cosmos); the doc that promised otherwise is corrected | fix/preaudit-staking | e342d670 | n/a | self | documented |
| S-006 | state/undo.rs | medium | The undo journal refuses a previous value above 16 MiB, and the staking record is journalled every block. Fixed: a staking transaction that would grow the record past 8 MiB is refused, so the transaction fails, never the block. Shrinking transactions and evidence are never refused, so a full record cannot block a slash | fix/preaudit-staking | e342d670 | none: the 8 MiB cap is reviewed, not tested (filling it is too slow for CI) | self | fixed |
| S-007 | fee-market/limits.rs | low | The fee floor of 1 base unit is 10⁶ cheaper at 24 decimals | fix/preaudit-chaintag | 299acd9d (#236) | n/a (runbook mainnet parameter) | self (not independent) | documented |
| S-008 | fee-market/supply.rs | info | The 21M cap is a genesis input, not a protocol rule (ADR-046 decision 3) | fix/preaudit-chaintag | 299acd9d (#236) | n/a (runbook mainnet parameter) | self (not independent) | documented |
| S-009 | consensus/bft/certsync.rs | low | A low head reported by a peer ends catch-up early (liveness only). Fixed: a peer behind this node is refused, so the node tries the next source | fix/preaudit-certsync | pending | `a_peer_behind_this_node_does_not_end_catch_up_as_success` | self (not independent) | fixed |
| S-010 | consensus/bft/certsync.rs | low | Per-page size is not capped on the client side; relies on the 10 MiB transport limit. Fixed: the client checks each page against its byte budget | fix/preaudit-certsync | pending | `a_page_over_its_byte_budget_is_refused_by_the_client` | self (not independent) | fixed |
| S-011 | consensus/bft/attest.rs, catchup.rs | high | With more than f nodes following at once (attacknet attack 8), checkpoints fall short of the n − f quorum followers count, so they stall and one exits; a 3-signature checkpoint formed in a 7-seat epoch, so server and follower may disagree on the epoch's committee | test/attacknet-restart-ahead | pending | attacknet attack 8 (`reports/attacknet/2026-10-29-1.md`) | self | open |
