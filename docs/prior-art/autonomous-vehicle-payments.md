# Prior art: autonomous-vehicle payments (vehicles pay tolls and charging inside limits they cannot exceed)

**Search status:** searched 2026-11-10 (web search; snippets only, primary pages not fetched; weights stated per row).

Feature: CATALOG §I row `autonomous-vehicle-payments` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R1 idea.

**Queries:** "IOTA Bosch mobility autonomous vehicle payments Mobility Open Blockchain Initiative MOBI tolls charging".

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| IOTA with MOBI and Bosch (2018) | IOTA joined the Mobility Open Blockchain Initiative in May 2018, with Bosch and automakers. Bosch's venture arm bought IOTA tokens. Example use given: cars paying for platooning fuel savings. Announcement and trade coverage, no production deployment found. (P/S, snippets) | [IOTA blog on MOBI](https://blog.iota.org/iota-joins-the-mobility-open-blockchain-initiative-mobi-1c017a8c00f0); [Calcalist on Bosch](https://www.calcalistech.com/ctech/articles/0,7340,L-3731506,00.html) |
| Jaguar Land Rover with IOTA "Smart Wallet" (concept) | Stated aim: drivers pay tolls, parking and EV charging automatically from rewards. Promotional Medium post; no deployment seen. (S) | [Medium, IOTA, Jaguar and the future of mobility](https://medium.com/thedarkside/iota-jaguar-and-the-future-of-mobility-ecd7d4ee603e) |
| Bosch peer-to-peer parking and charging on Peaq (IAA Mobility 2023) | Per a search summary, Bosch demonstrated parking and charging payments using moveIDs on the Peaq network, not IOTA. **No URL retained; fetch a primary page before citing.** | Unverified; not fetched |

Gap: no confirmed production toll or charging system on IOTA with Bosch was found.

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/smart-account/src/policy.rs`: `Policy { daily_limit, per_recipient_daily, allow_list, delay_above, delay_blocks }`. The header says limits are "evaluated by the chain, not by app code". `BLOCKS_PER_DAY = 43_200` at 2 s blocks.
- `crates/smart-account/src/policy.rs`: `SessionScope { recipients, max_amount, expires_at }` with `allows(to, amount, height)`. Heights, not timestamps (invariant 9).
- [features.toml](../../features.toml) line 3433: "Vault accounts: delayed, guardian-cancellable withdrawals" is class REAL, status verified (`crates/node/src/state/vault.rs`).
- `hal/swarm`: MAVLink 2 and ROS 2 CDR, sealed task auctions, committed flight plans. Class RESEARCH, status working ([features.toml](../../features.toml) line 2385). Drones and robots, not road vehicles.
- `rg -il "toll|vehicle|charging|ocpp|iso 15118" crates hal bins`: one hit, `hal/swarm/src/mavlink.rs`. No OCPP or ISO 15118 code.

## How a Maya2C version would differ

- **Limits enforced by consensus, not the vehicle.** A compromised vehicle controller still cannot pay past its session `max_amount` or `expires_at`, because the chain checks `SessionScope::allows`. The IOTA and JLR material shows no chain-enforced cap of this kind in the sources read. Design claim, not built.
- **Delay for large payments.** `delay_above` and `delay_blocks` give a cancel window for big transfers. Cost: a payment above the threshold is not instant.
- **Scope of the claim.** The safety of a vehicle is not the chain's job. A payment limit does not make a car safe; that is a certification question.

**Legal and ethical gates.** Safety certification for any vehicle function, not just payment: the relevant automotive functional-safety and cybersecurity standards were **not searched**; name the standard before any claim. Payment and road-charging regulation by country. Driver location data in toll records is personal data (GDPR, see [emergency-identity.md](emergency-identity.md) for the counsel question).

**Verdict: similar exists as consortium work and concepts (IOTA, Bosch, MOBI, JLR). No production deployment found. Maya2C's chain-enforced session limits are a design point; the vehicle itself is outside the chain's scope and needs certification before any claim.**

**forbidden**: "first vehicle payment chain", "only self-driving payments", "unprecedented toll payments", "safe autonomous payments".
