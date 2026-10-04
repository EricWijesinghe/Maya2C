# Runbook: Chain halt (liveness failure)

**Symptom.** `ProductionStalled` fires: no block imported for 150 s.

**Check.** Is it this node (see fell-behind) or everyone? Check two independent explorers or operators.

**Act.** If network-wide: coordinated restart from an agreed height and state root (docs/runbooks/coordinated-restart.md).

**Rehearsal.** rehearsed: `crates/node/tests/coordinated_restart_rehearsal.rs`

## DAG-BFT: the committee itself cannot form a quorum

**Symptom.** Every honest node is up and connected, rounds stop advancing,
and `get_bft_status` shows the same `committed_round` everywhere. Usually
one or more committee members have stopped answering for good. Under
equal-weight voting that can be cheap registrations that never came online
(ADR-039).

**Why a restart does not help.** Each node rebuilds the same committee from
the same state, so the same absent members still hold their seats.
Downtime is judged only at an epoch boundary, and a halted chain never
reaches one.

**Act, today.**
1. Confirm, through at least two operators, the last block every honest
   node holds and its state root.
2. Find the absent members: compare the committee
   (`get_bft_status`.`committee`, and the epoch's `k:cmt:` record) with the
   validators actually connected.
3. There is no supported way yet to remove them in place. The recovery is a
   new genesis that carries the balances forward, which is a re-genesis.
   On a testnet, announce it and re-genesis. On mainnet this is gate 10:
   stake-weighted voting (ADR-040) makes the halt need a third of the
   *stake*, and a recovery path is still to be designed.

**Prevent.** On a testnet run by one operator, start the validator with
`--min-register-bond` above what the faucet can fund (ADR-039 option 3).

## The only validator's host restarted

**Seen 2026-10-04.** Windows Update restarted the testnet PC at 12:59:20 to
install an Insider build (`MoUsoCoreWorker.exe`, User32 event 1074 in
`C:\Windows.old\…\System.evtx`). The upgrade took until 15:38. The
watchdog and logon script brought the stack back by 15:44:28. There were
no blocks for 2 h 45 min.

**Check.** Run the block-gap scan against the local node: look for the
largest timestamp gap between consecutive heights. Read the System event
log, or the previous OS's log after an upgrade, for event 1074 at the gap's
start.

**Prevent.** One validator on a desktop halts with every OS update. Until
the seed moves to an always-on server, the owner pauses Windows Update or
sets active hours on that PC. Claude does not change OS update settings.
