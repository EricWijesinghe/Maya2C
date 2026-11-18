# Controller runs: not evidence

Written 2026-11-10. The runs under `runs/` (2026-10-09 and 2026-10-10,
scenarios 1.1, 1.8 and 1.9) came from `bins/attacknet-controller` before the
`fix/attacknet-honesty` change. They are kept as history, not as evidence for
any gate:

- **1.8's PASS was hard-coded.** `verify_quorum_behavior` returned `Ok(true)`
  without checking anything. It now returns an error.
- **Metrics were never collected.** The controller probed
  `GET /get_tip_height`; the node's RPC is JSON-RPC over POST, so every
  metric reads 0. The probe is now a POST.
- **1.9's partition is not known to have been applied.** netsh's exit status
  was ignored (and `name=` was passed apart from the rule name), so nothing
  shows a firewall rule existed; the run records 0 partitions. netsh failures
  are now errors.

Gate evidence is `cargo xtask attacknet` only: the dated reports one
directory up (`reports/attacknet/*.md`).
