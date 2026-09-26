# Announcement copy: draft

**Status: DRAFT, waiting for `APPROVED: announce`.** Every claim below is
checked by `cargo xtask claims-check`, which fails on a superlative that no
completed prior-art search permits. The copy deliberately makes no "first"
or "only" claim: the one search that bears on the headline
([pq-from-genesis](../prior-art/pq-from-genesis.md)) forbids "first
post-quantum L1", because QRL has been post-quantum since 2018.

---

**Maya2C: hybrid post-quantum signatures on every transaction from genesis.**

Every Maya2C transaction is signed twice: once with ML-DSA-65 (lattice) and
once with SLH-DSA (hash-based). Both must verify. If either family is broken,
the other still holds. That protection costs something, and we publish the
cost:

- a transfer is 13 KB on the wire;
- verifying one takes about a millisecond on one core, roughly 17× an
  Ed25519 check.

See the [benchmark report](BENCHMARK_REPORT.md).

What you can build on today:

- **Accounts without seed phrases.** Keys can be rotated, and guardian
  recovery comes with an owner veto window.
- **Spending keys that cannot overspend.** Point-of-sale session keys are
  scoped to one merchant, one amount and one expiry.
- **Vaults with time to react.** Large withdrawals wait, and guardians can
  cancel.
- **Wallet reviews that read the simulation, not the app's description.**

What is not ready, stated as plainly:

- There is no public testnet.
- Contracts cannot yet check who called them, so ownership contracts wait
  for ADR-026.
- The fee market is inactive.
- Nothing has been audited externally.

Where Maya2C stands against the best public numbers, including where it is
worse: [beat-bar status](BEAT_BAR_STATUS.md).
