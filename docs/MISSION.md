# Mission

Maya2C is a post-quantum Layer-1 blockchain and the base layer of an
ecosystem of separate applications: chat first, then a wallet, then AI
services, then others. It is owned by Eric and built in `D:\Maya2C`.

## The one thing that decides success

A chain earns trust by **never losing funds, never forking unexpectedly, and
publishing numbers outsiders can reproduce.** Every rule in
[CLAUDE.md](../CLAUDE.md) serves one of those three. When a rule conflicts
with speed, the rule wins.

## The milestone ladder

The single source of truth for ordering. Detail, the Master Prompt mapping
and each milestone's exit criteria: [EMPIRE.md](EMPIRE.md). Status is
derived from evidence and recorded in [STATE.md](../STATE.md); nothing
advances a milestone until the previous one's criteria are met and recorded.

| | Milestone | Means |
|---|---|---|
| M0 | TRUTH | Master Prompt 11 complete. We know exactly what works. |
| M1 | CORE SOLID | Core builds clean; state, fees, consensus, VM have passing tests, property tests and fuzzing; formal invariants proven or listed open. |
| M2 | FAST AND PROVEN | Performance and post-quantum weight measured and published internally. |
| M3 | USABLE | Smart accounts, safe contracts, developer platform, wallet and explorer usable by outsiders. |
| M4 | CONNECTED | Proof-verified interoperability and the integration layer. |
| M5 | OPERABLE | Security, spec and conformance, validator key handling, operations and runbooks. |
| M6 | PUBLIC TESTNET | Open testnet with external validators; attack round survived; beat bars measured. |
| M7 | AUDITED | External audits complete; no open critical or high findings. |
| M8 | MAINNET | Go/no-go passed; genesis; first-90-days plan active. |
| M9 | ECOSYSTEM | Chat, then wallet integration, then AI services — separate apps on a shared identity ([ECOSYSTEM.md](ECOSYSTEM.md)). |

## Where the rest lives

| Question | Read |
|---|---|
| How every session runs | CLAUDE.md, "Operating Protocol" |
| Where we are now | [STATE.md](../STATE.md), `cargo xtask status` |
| What was decided, and why | [DECISIONS.md](../DECISIONS.md), [docs/adr/](adr/) |
| What is waiting | [BACKLOG.md](../BACKLOG.md) |
| What could sink it | [RISKS.md](../RISKS.md) |
| What exists and whether it works | `features.toml`, `cargo xtask coverage` |
