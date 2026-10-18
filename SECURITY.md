# Security policy

## Reporting a vulnerability

**Do not open a public issue for a security problem.** Report privately
through GitHub's *Report a vulnerability* (private vulnerability reporting) on
this repository: <https://github.com/EricWijesinghe/Maya2C/security/advisories/new>.

Include what you found, the steps to reproduce it, the commit or release you
tested, and what you think an attacker could do with it. A proof of concept
helps, but a clear description without one is still accepted.

## Response targets

These are targets for a one-maintainer project, not guarantees.

| Step | Target |
|---|---|
| Acknowledgement | within 3 working days |
| Initial assessment (valid, invalid, or needs more detail) | within 10 working days |
| Status update while open | at least every 14 days |
| Fix for Critical or High | as fast as a safe patch allows, coordinated with validators (see severity table) |
| Public advisory | after a fix has shipped, or 90 days after the report, whichever is sooner |

Please give us 90 days, or until a fix has shipped to validators, before
publishing. We credit reporters in the advisory unless you ask us not to.

## Scope

In scope: code and services in this repository on a production path.

- **Node and consensus:** `crates/node`, `bins/maya2c-node`, `crates/dag-bft`,
  `crates/state-sync`, `crates/light-client`.
- **Cryptography:** `crates/crypto-pq`, `crates/zk-stark`, `crates/custody-mpc`,
  `crates/signer`.
- **VM and ledger arithmetic:** `crates/vm`, `crates/ledger-math`,
  `crates/amount`.
- **Wallet:** `apps/wallet-gui` (including its core), and the Ledger app
  `apps/ledger-maya2c`.
- **APIs and services:** `crates/api-gateway` (RPC), `apps/faucet`,
  `crates/stratum-v2` (mining port), and the peer-to-peer transport.
- **Websites:** the documentation site in `docs/site` and the sites under
  `apps/portal`, `apps/explorer` and `apps/status`.

Out of scope:

- Anything the reality ledger (`features.toml`) marks `SIM` or `RESEARCH`,
  *unless* you can show it reachable from a production path. That reachability
  is itself a finding, and it is welcome.
- Denial of service by volume alone against one node, or against shared
  testnet infrastructure.
- Findings that need a host you do not control, or a compromised operator.
- Social engineering, physical attacks, and attacks on third-party services.
- Automated scanner output with no reproduction. A scanner result is a lead,
  not a report.
- Known issues listed below.

## Rules of engagement

- Test on your own local devnet, or on the public testnet. Do not test against
  anyone else's funds or accounts.
- Do not access, modify or delete data that is not yours. Stop and report if you
  reach it by accident.
- Do not degrade a shared service for other users.

## Safe harbour

If you make a good-faith effort to follow this policy, we will not start or
support legal action against you for security research on the in-scope systems,
and we will treat your report as research rather than as an attack. This
applies to actions taken within the rules above. It does not cover breaking
any law unrelated to this project. The maintainer is not a lawyer; a counsel
review of this wording is pending (see `docs/legal/`).

## Severity

| Level | Examples | Target fix |
|---|---|---|
| Critical | value created or destroyed, consensus split, key extraction | patch to validators before disclosure (docs/security/INCIDENT_RESPONSE.md) |
| High | remote node crash, signature bypass on a non-default path | next release |
| Medium | resource exhaustion with amplification, information leak | scheduled release |
| Low | hardening | backlog |

## Recognition

Reporters of valid vulnerabilities are listed in the
[Security Hall of Fame](https://maya2c.dev/guides/security-hall-of-fame/),
with your consent. Contributor badges, including the security one, are described
on the [Contributors & badges](https://maya2c.dev/guides/contributors/) page.

**There are no cash rewards yet.** A bounty programme is planned but not live.
The draft scope and reward table in `docs/security/BUG_BOUNTY.md` is not
published, and no amount is promised. Rewards will be offered once funding
exists, and this file will say so when they are.

## Known issues

Tracked honestly in `THREAT_MODEL.md` (the **Gap** column) and in
`reports/`. The standing ones: HQC decapsulation is not constant-time
(ADR-009), and the shielded-pool circuit has not been externally audited
(`CIRCUIT_IS_AUDITED = false`).
