# Runbook: Deep reorganisation

**Symptom.** Tip switches back many blocks.

**Check.** Depth of the reorg; attacker hash-rate estimate.

**Act.** Exchanges: raise confirmation depth (docs/integrations/EXCHANGES.md table). Below the prune horizon a reorg is refused by design.

**Deposit watcher (`crates/deposit-watcher`).** It credits heights the operator marks finalized, and it has no reorg handling: a credit already fsync'd to its journal is never reversed by the watcher. So the confirmation depth it is given must be deeper than any reorg you expect. If a reorg reaches a height the watcher already credited:
- Stop the watcher. Do not delete or edit the journal; it is the evidence.
- List the credits from the reverted heights (each journal line is `height` plus `credits`). There is no rollback command in the repo; any reversal is a manual, recorded step.
- Amounts are 256-bit base units, 10^24 per coin, written as base-10 strings. Read them as text; a value above `u64::MAX` is normal, not corruption. A journal line that cannot be parsed stops the watcher with `Corrupt` by design.

**Rehearsal.** pinned: undo-journal reorg tests; not rehearsed as an incident
