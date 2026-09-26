# Runbook: State root mismatch

**Symptom.** Block rejected with `state root mismatch`.

**Check.** Is it one node or many? Compare the node's root at the parent with two peers.

**Act.** One node: its database is wrong (see db-corruption). Many: a consensus bug; stop and coordinate (chain-halt).

**Rehearsal.** pinned by invariant 24 tests; not rehearsed as an incident
