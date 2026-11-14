# Threat models

One file per feature, written before the build lane starts and read by it.
Each has the same five parts: attacker, assets, attacks, mitigations, and a
replay-test list whose names the build lane uses verbatim as red tests
(`crates/node/tests/<feature>_tests.rs`). A test on the list that cannot be
written is reported as `NOT VERIFIED: <reason>` in the build PR, never
dropped quietly.

| File | Feature | ADR | Status |
|---|---|---|---|
| [v4-tokens.md](v4-tokens.md) | Token mint/burn (P-1) | [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-1 | written 2026-11-12, tests pending |
| [v4-nfts.md](v4-nfts.md) | Unique assets (P-2) | ADR-135 §P-2 | written 2026-11-12, tests pending |
| [v4-names.md](v4-names.md) | Names (P-3) | ADR-135 §P-3 | written 2026-11-12, tests pending |
| [v4-paymaster.md](v4-paymaster.md) | Sponsored fees (P-4) | ADR-135 §P-4 | written 2026-11-12, tests pending |
| [v4-smart-accounts.md](v4-smart-accounts.md) | Smart-account policy (P-5) | ADR-135 §P-5 | written 2026-11-12, tests pending |
| [v4-scheduled-payments.md](v4-scheduled-payments.md) | Scheduled payments (P-7) | ADR-135 §P-7 | written 2026-11-12, tests pending |

Common attacker model for all six: anyone who can submit transactions, run
a node, or produce blocks (a committee member under ADR-040). The attacker
cannot forge a registry-suite signature and cannot make two honest nodes
read different committed state. The attacker *can* reorder, delay or drop
transactions in blocks it produces, hold many keys, and watch the mempool.
