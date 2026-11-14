# Prior art: Energy credits (measured, verifiable energy sharing between neighbours)

**Search status:** searched 2026-11-10 (web search and page fetches; sources below). Current regulation by country was not surveyed.

Feature: CATALOG §I row `energy-credits`, from `hal/energy` and `iot-anchor`.

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Power Ledger | Peer-to-peer energy trading and traceability, now on Solana; deployments cited in Austria, Greece, Thailand | [powerledger.io](https://www.powerledger.io/) |
| Energy Web | Verification infrastructure for energy and environmental markets (Green Proofs registries) | [energyweb.org](https://www.energyweb.org/) |
| LO3 Energy / Brooklyn Microgrid | Ethereum-based neighbour solar trading with ConsenSys, ~500 participants by 2017, Siemens microgrid controls | [MIT Technology Review](https://www.technologyreview.com/2017/04/19/243021/blockchain-is-helping-to-build-a-new-kind-of-energy-grid), [Utility Dive](https://utilitydive.com/news/siemens-lo3-energy-teams-up-for-blockchain-powered-microgrid-in-brooklyn/430884) |

Not found: LO3's current status (sources are 2017 era).

## How a Maya2C version would differ (each backed by the repo)

- **Gateway-side protocol parsers:** `hal/energy` parses Modbus/TCP and IEC 61850 GOOSE and encodes IEEE 1547 trip rules (`hal/energy/src/lib.rs:1-10`), tested on spec examples and hostile input; the GOOSE test frame is hand-encoded, not captured from a relay.
- **The grid is SIM** and the market prices given samples (`lib.rs:7-9`, `12-14`); the node does not link the crate; no real meter connected.
- **Device identity:** `hal/iot-anchor` records an ML-DSA-65 signed Merkle root over readings and says it "does **not** prove the readings are physically true: a heated thermometer signs honestly" (`hal/iot-anchor/src/lib.rs:1-9`). An energy credit inherits that limit.
- **Small payments:** `crates/l2-flash` barter / SLA netting is a candidate settlement route; the energy crate does not call it.
- **Activation:** `IOT_ACTIVATION_HEIGHT = u64::MAX` (`crates/node/src/state/context.rs:103`), so the IoT subsystem never activates; the node does not link `hal/energy`.

## Legal and ethical gates

- Metering law: EU Measuring Instruments Directive 2014/32/EU ([EUR-Lex summary](https://eur-lex.europa.eu/EN/legal-content/summary/eu-rules-ensuring-that-measuring-instruments-are-accurate.html)); billing-grade claims need a certified meter.
- Utility and market licensing for trading energy. Not researched here.
- Possible securities or commodity treatment of credits (lawyer's call).
- Grid interconnection rules (IEEE 1547). Civil use only; read side only, no grid control.

## What may be said publicly

- Accurate: "Maya2C includes tested Modbus and IEC 61850 parsers and a simulated energy market; no live meter is connected."
- **forbidden**: "first energy trading blockchain", "only verifiable energy credits", "unprecedented", "tamper-proof meter data", "billing-grade".

**Verdict:** RESEARCH, simulated. Promotion would need a certified meter feed, an answer to the "signed but not true" gap, and metering and market-licence advice per jurisdiction.
