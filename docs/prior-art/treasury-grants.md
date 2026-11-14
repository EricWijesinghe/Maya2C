# Prior art: Treasury spend path and grants

**Search status:** partially searched 2026-11-12 (WebSearch, 2 queries: "Cardano treasury withdrawal governance action CIP-1694 Polkadot OpenGov treasury spend track Optimism RetroPGF"; "Polkadot OpenGov treasury tracks small spender medium spender conviction voting wiki"). Optimism RetroPGF returned no result and is **not searched**; it is listed from general knowledge.

Feature: parity row P-28, CATALOG rows `treasury-ledger` and `grants-quadratic`, [ADR-111](../adr/ADR-111-treasury-spend-and-grants.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Cardano (CIP-1694, Conway era, live since 2025-01-29) | A treasury withdrawal is a governance action paying ada to a stake address; it needs DRep approval (about 67% per one DRep rationale) and Constitutional Committee sign-off, with a one-epoch enactment delay; a net-change limit caps the total per period; a failed action moves nothing and its deposit is handled by the rules. | [cardano.org glossary: treasury withdrawal](https://cardano.org/glossary/treasury-withdrawal/), [CIP-1694](https://cips.cardano.org/cip/CIP-1694), [first disbursements (secondary)](https://www.cryptopolitan.com/de/cardano-community-treasury-disbursements/) |
| Polkadot OpenGov | Treasury requests are routed by size through tracks (Small Tipper, Big Tipper, Small Spender, Medium Spender, Big Spender, Treasurer), each an origin with its own approval and support curves; treasury proposals were about 81% of referenda in an early report. | [docs.polkadot.com origins and tracks](https://docs.polkadot.com/reference/governance/origins-tracks/), [Parity OpenGov report](https://data.parity.io/opengov_report.pdf) |
| Optimism RetroPGF | Retroactive rounds decided off chain by badgeholders, paid on chain. **Not searched** this round. | — |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/treasury`: staged spending with a per-epoch limit and a ledger (`spending.rs`), vesting grants (`vesting.rs`); tested, not wired (`lib.rs:1-7`); `features.toml` treasury-vesting-rules RESEARCH.
- Genesis may fund a treasury account up to 50% of supply (`crates/node/src/genesis.rs:461-511`); mainnet funds none (`deploy/genesis/mainnet/README.md:72`); the fee split's treasury share is 0 (`genesis.rs:277`; `crates/fee-market/src/split.rs`, `MAX_TREASURY_BPS = 5_000`).
- Governance proposals carry parameter changes and upgrade schedules only (`crates/node/src/core/governance_payload.rs:148-157`); compiled floors (`crates/governance/src/limits.rs`, invariant 12).

## How the Maya2C version differs

- **One lifecycle**: a spend is a field on the existing proposal and passes the same vote, timelock and execution as a parameter change; no tracks, no second body.
- **Compiled epoch cap** (`TREASURY_EPOCH_CAP_BPS`) that governance cannot raise, in the spirit of invariant 12; Cardano's net-change limit and Polkadot's track limits are themselves governable.
- **No key for the treasury**: a derived address, so the spend path is the only exit.
- **Vested grants as chain records** the grantee claims by height, with claw-back by a later proposal; the surveyed chains pay in full at enactment.
- **No quadratic matching** until proof of personhood exists; stated, not promised.

**Verdict: similar exists (Cardano withdrawals, Polkadot tracks, RetroPGF). Differentiate by the compiled cap and vested grant records. No "first" claim.**
