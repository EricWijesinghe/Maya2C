# Runbook: Deep reorganisation

**Symptom.** Tip switches back many blocks.

**Check.** Depth of the reorg; attacker hash-rate estimate.

**Act.** Exchanges: raise confirmation depth (docs/integrations/EXCHANGES.md table). Below the prune horizon a reorg is refused by design.

**Rehearsal.** pinned: undo-journal reorg tests; not rehearsed as an incident
