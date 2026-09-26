# Runbook: Mempool full

**Symptom.** New transactions refused; users see stuck sends.

**Check.** `maya_mempool_transactions` at capacity (4,096 default).

**Act.** Raise capacity if memory allows; under the fee market, eviction by fee per byte applies (dos-guard admission, not yet in the node).

**Rehearsal.** not rehearsed
