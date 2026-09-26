# 27 — Privacy as a developer primitive

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 27

> DONE WHEN: private state and functions work in contracts; viewing keys
> and association-set proofs pass tests; mobile proving is measured; the
> privacy limits document exists; reports/27-privacy.md is complete.

| Condition | Result |
|---|---|
| Private state and functions in contracts | **not built** |
| Viewing keys pass tests | **yes**, `crates/privacy` (per account and per transaction) |
| Association-set proofs pass tests | **the exclusion half, yes**: "not from a flagged set" is the existing STARK in `maya-zk-stark::sanctions`, measured here. Inclusion in an approved set is **not built** |
| Mobile proving measured | **no.** Desktop only, recorded |
| Privacy limits document | `docs/privacy/LIMITS.md` |

```
$ cargo test -p maya-privacy --profile ci -- --nocapture
test the_account_viewing_key_opens_its_envelopes_and_no_one_elses ... ok
test a_disclosure_key_opens_one_transaction_and_not_the_accounts_others ... ok
test a_tampered_envelope_does_not_open ... ok
non-membership in a 1,000-entry flagged set: prove 72 ms, verify 46.8 ms, proof 731521 bytes (debug assertions on; desktop 4 vCPU, not a phone)
test not_from_a_flagged_set_proof_is_measured ... ok
test result: ok. 4 passed; 0 failed …
```

**Viewing keys.** A shielded transfer's details are sealed with a fresh
ML-KEM-768 encapsulation to the account's viewing key, plus
ChaCha20-Poly1305. There are two levels of disclosure:

- the account viewing key opens that account's envelopes and no one else's;
- a per-transaction disclosure key opens exactly one envelope.

A one-bit change to an envelope makes it refuse to open. Everything here is
post-quantum.

**What the measurement says.** A non-membership proof is **731 KB**, about
55× a hybrid transfer. It does not fit on the chain per transaction as it
stands. Proof compression or recursion, or proving against an aggregated
set, is the open work before association-set proofs can be a normal
option.

**Not done.**

- Private contract state and functions, and SDK support for them.
- Approved-set *inclusion* proofs (the Privacy Pools style), and a
  documented comparison with Privacy Pools (not searched:
  `docs/prior-art/privacy-viewing-keys.md`).
- Shielded transfers in a wallet.
- Mobile proving.
- An external cryptography audit, which is required before activation and
  is already in audit scope (a) (`docs/audit/README.md`).
