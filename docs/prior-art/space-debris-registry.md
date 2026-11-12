# Prior art: space-debris registry (one public registry of orbital objects with owners and manoeuvre history)

**Search status:** searched 2026-11-10 (web search; snippets only, treaty texts not fetched; weights stated per row).

Feature: CATALOG §I row `space-debris-registry` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R2 idea.

**Queries:** "UN Register of Objects Launched into Outer Space blockchain space debris registry LeoLabs Space-Track".

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| UN Register of Objects Launched into Outer Space (UNOOSA) | Kept since 1962. States and intergovernmental organisations under the Registration Convention keep national registries and submit to the UN register. UNOOSA notice: the online index and export are currently unavailable because of IT changes. (P, snippet) | [UNOOSA register](https://www.unoosa.org/oosa/en/spaceobjectregister/index.html) |
| LeoLabs | Commercial radar tracking of orbital objects, sold as a subscription. It is a tracking service, not a registry. (S, snippets; the older object count was not used) | [LeoLabs, $4M round](https://leolabs.space/news/leolabs-closes-4m-funding-round/) |
| Space-Track | US tracking catalogue. The results cite it only in a 2018 paper; its current role and data were not verified. | Unverified; not fetched |
| Blockchain registration proposal (academic) | Proposes scenarios for registering space objects on a blockchain that fit both technical and legal requirements. A proposal; no deployed system. (academic, snippet) | [eustu-2023-0020](https://reference-global.com/2/v2/download/article/10.2478/eustu-2023-0020.pdf) |

Gap: no blockchain-based UN, LeoLabs or Space-Track registry was found.

## What Maya2C has (checked in repo, 2026-11-10)

- `hal/link-sim/src/lib.rs` line 20 declares `pub mod orbital`. `hal/link-sim/Cargo.toml` line 8 lists "orbital delay" among SIM and RESEARCH link models. That is a link-latency model, not an object registry.
- `rg -i "debris|norad|conjunction|orbital" crates hal bins`: hits only in `hal/link-sim`. No orbital-object schema, no tracking-data ingest.
- `crates/rwa` ("Real-world asset primitives", class RESEARCH, status working): [features.toml](../../features.toml) line 2622. A record store for assets; no orbital fields.
- `crates/da` (2D Reed-Solomon and sampling, RESEARCH, working): [features.toml](../../features.toml) line 3105. Stores and proves data availability, not object history.

## How a Maya2C version would differ

- **Mirror, not authority.** Ownership and registration are state acts under the Registration Convention. A Maya2C registry could only hold a hash-linked, height-stamped mirror of records that states file. The chain does not decide who owns an object.
- **Tamper evidence for manoeuvre history.** Each manoeuvre notice would be a signed record at a height. This adds an audit trail; it does not add tracking data.
- **The hard input is the data.** Positions come from radar and optical operators (LeoLabs-type services or government catalogues). Maya2C has no ingest path and no source licence. Design claim only.

**Legal and ethical gates.** Outer Space Treaty: Article VI makes states responsible for national activities in space, including by non-governmental entities; Article VIII keeps jurisdiction and control with the registering state. The Liability Convention and the Registration Convention also apply. These texts were **not fetched**; cite them from the primary treaty text before publication. Licensing of orbital data sources. Export controls on tracking data.

**Verdict: similar exists (UN register, commercial trackers, an academic blockchain proposal). Maya2C has no orbital data path; the R2 rung holds. A mirror of state-filed records is the only defensible shape, and it is not built.**

**forbidden**: "first space registry on a blockchain", "only debris registry", "unprecedented orbital ledger", "tracks space debris".
