# maya2c-validator-ledger

A public record of validator participation, built only from signed consensus
data that a public endpoint serves (invariant 14: never from self-reports).

```sh
maya2c-validator-ledger --node https://<gateway>/rpc [--epochs 8] [--json]
```

For each of the last N epochs it prints which committee indices signed the
epoch's final quorum-attested checkpoint (`get_checkpoint [epoch]`), and for the
current epoch which indices are not on the newest checkpoint held (committee
size from `get_bft_status`). That is a missing signature, not a missed round.

Every output ends with a MISSING list. Through the gateway there is no public
source for: stake (`get_validators` is not allowlisted, and no RPC serves the
epoch-frozen weights), which validator holds an index (keys are not public, so
signatures are counted, not verified), rounds per validator (`get_certificates`
is not allowlisted), uptime, equivocation evidence, and epochs the endpoint no
longer holds. Those columns are absent, never zero.
