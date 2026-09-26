# Security policy

## Reporting a vulnerability

**Do not open a public issue for a security problem.** Report privately
through GitHub's *Report a vulnerability* (Security Advisories) on this
repository. Include what you found, how to reproduce it, and the commit you
tested. You will get an acknowledgement within 3 working days and an initial
assessment within 10.

Please give us 90 days, or until a fix has shipped to validators, before
publishing — whichever is sooner. We will credit you in the advisory unless you
ask us not to.

## Scope

In scope: everything in this repository that is on a production path —
the node (`crates/node`, `bins/maya2c-node`), cryptography (`crates/crypto-pq`,
`crates/zk-stark`, `crates/custody-mpc`), the VM (`crates/vm`), the wallet core
and the Ledger app, the API gateway and faucet.

Out of scope: anything the reality ledger (`features.toml`) marks `SIM` or
`RESEARCH`, *unless* you can show it reachable from a production path — that
reachability is itself a vulnerability. Denial of service by volume alone
against a single node. Findings that need a compromised host.

## Severity

| Level | Examples | Target fix |
|---|---|---|
| Critical | value created or destroyed, consensus split, key extraction | patch to validators before disclosure (docs/security/INCIDENT_RESPONSE.md) |
| High | remote node crash, signature bypass on a non-default path | next release |
| Medium | resource exhaustion with amplification, information leak | scheduled release |
| Low | hardening | backlog |

## Bug bounty

A bounty programme is **planned, not live**. The draft scope and rewards
table is in `docs/security/BUG_BOUNTY.md`; publishing it needs explicit
approval (Standing Order 6). Until then, reports are welcome and credited, but
no reward is promised.

## Known issues

Tracked honestly in `THREAT_MODEL.md` (the **Gap** column) and in
`reports/`. The standing ones: HQC decapsulation is not constant-time
(ADR-009), and the shielded-pool circuit has not been externally audited
(`CIRCUIT_IS_AUDITED = false`).
