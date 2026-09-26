# Bug bounty — draft, not published

Master Prompt 16 §6. **Status: ready for review, not live.** Publishing
requires `APPROVED: bug-bounty` (Standing Order 6), and funding it is a
treasury decision.

## Scope

Same as SECURITY.md: production paths only; SIM/RESEARCH modules only when
shown reachable from a production path.

## Rewards (draft; denominated in a stable unit, paid after the fix ships)

| Severity | Example | Reward range |
|---|---|---|
| Critical | value created or stolen; consensus split reproducible from the network | 50,000 – 250,000 |
| High | remote crash of all nodes of a version; signature bypass on a non-default path | 10,000 – 50,000 |
| Medium | DoS with amplification; key-material leak requiring local access | 2,000 – 10,000 |
| Low | hardening findings with a concrete exploit path | 250 – 2,000 |

## Rules

- First valid report wins; duplicates are credited, not paid.
- No testing against mainnet or other people's funds; use a local devnet
  (`scripts/local_cluster.sh`) or the public testnet once it exists.
- No social engineering, no physical attacks, no DoS against shared
  infrastructure.
- Give us the disclosure window in SECURITY.md.
- Sanctioned persons and jurisdictions cannot be paid; this is a legal
  constraint on the payer, and counsel must confirm the list (docs/legal/).
