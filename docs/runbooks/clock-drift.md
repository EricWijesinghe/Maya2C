# Runbook: Clock drift

**Symptom.** Blocks rejected for timestamps; peers disagree on tip age.

**Check.** `chronyc tracking`; offset against two NTP pools.

**Act.** Fix NTP. Consensus uses heights, not wall-clock time (invariant 9); drift affects block timestamps and monitoring, not balances.

**Rehearsal.** not rehearsed; Marzullo fusion in `crates/timing` is tested
