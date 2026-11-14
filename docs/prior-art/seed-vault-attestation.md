# Prior art: seed-vault attestation (proof that a crop seed sample is held, viable and genetically distinct)

**Search status:** searched 2026-11-10 (web search; snippets only, primary pages not fetched; weights stated per row).

Feature: CATALOG §I row `seed-vault-attestation` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R1 idea.

**Queries:** "Svalbard Global Seed Vault DivSeek blockchain seed bank provenance attestation"; "Nagoya Protocol digital sequence information DivSeek ITPGRFA multilateral system seed genetic resources".

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| Svalbard Global Seed Vault | Holds safety duplicates. Depositors keep ownership. Run under a tripartite agreement (Norwegian government, Crop Trust, NordGen). Late 2021: 1,125,419 duplicates from 89 genebanks. No blockchain attestation found. (P, snippet) | [NordGen annual progress report 2021](https://www.nordgen.org/media/gnnpp1i0/annual-progress-report-2021.pdf) |
| arXiv 2501.15962 | Proof-of-concept pairing genebank storage conditions with a blockchain lottery that rewards user participation. Names Svalbard as an example genebank. Its authors list open technical issues. (academic, snippet) | [arXiv 2501.15962](https://arxiv.org/pdf/2501.15962) |
| DivSeek | A 2017 Third World Network report says the ITPGRFA ended its association with DivSeek over benefit-sharing and farmer-rights concerns. Dated; not updated. (S, snippet) | [TWN, 2017](https://twn.my/title2/susagri/2017/sa587.htm) |
| ITPGRFA multilateral system | Standard material transfer agreement governs seed exchange; Nagoya is bilateral. Whether digital sequence information (DSI) falls under benefit-sharing is contested. Outcome of the 11th Governing Body (Lima) not found. (P/S, mixed) | [Frontiers 2019](https://www.frontiersin.org/articles/10.3389/fpls.2019.01046/pdf); [CBD DSI views, ITPGRFA](https://www.cbd.int/abs/DSI-views/ITPGRFA-DSI.pdf) |

Gap: no DivSeek, Svalbard or genebank blockchain attestation system in production was found.

## What Maya2C has (checked in repo, 2026-11-10)

- `rg -n -i "genotyp|seed.?bank|genebank|viabil" crates hal bins`: no hits. No seed or genetic-sample code.
- `crates/da` (2D Reed-Solomon and sampling, RESEARCH, working): [features.toml](../../features.toml) line 3105. Proves data availability, not a physical sample.
- `crates/archive`: historical archive, REAL, verified ([features.toml](../../features.toml) line 2024); forward-secure SLH-DSA seals at `crates/archive/src/seal.rs` and `reseal.rs` (entry at line 2985). These seal records, not objects.
- Genetic data is "data-sovereignty only" in the CATALOG ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md) line 110, `genetics-vault`). No code.

## How a Maya2C version would differ

- **Attests a record, not the seed.** A lab-signed custody record, hashed and sealed at a height. The chain can prove the record existed; it cannot prove the sample is viable or genetically distinct. Those are lab facts, which means an oracle input (invariant 11 is the oracle rule, cited in CLAUDE.md).
- **Forward-secure sealing.** Records sealed with the existing archive seal scheme would stay verifiable over decades. This is the existing `crates/archive` design applied to custody records. Design claim, not built.

**Legal and ethical gates.** Lab certification and biobank standards: **not searched**; name the standard before a claim. The ITPGRFA SMTA and Nagoya benefit-sharing rules, and the DSI question above. Putting a gene sequence or its hash on chain may create a benefit-sharing obligation; this is a counsel question, not settled. Access to Svalbard is governed by the depositing genebank, not Maya2C.

**Verdict: similar exists as an academic proof-of-concept (arXiv 2501.15962) and as the genebank system itself, which uses no public chain attestation in the sources read. Maya2C would attest lab records, not seed viability; not built, and DSI rules must be settled first.**

**forbidden**: "first seed bank on a blockchain", "only seed attestation", "unprecedented crop provenance", "proves seed viability on chain".
