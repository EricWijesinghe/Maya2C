# Prior art: emergency identity (portable, recoverable identity for people who lost their papers)

**Search status:** searched 2026-11-10 (web search; snippets only, primary pages not fetched; weights stated per row).

Feature: CATALOG §I row `emergency-identity` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R1 idea.

**Queries:** "UNHCR Building Blocks blockchain WFP cash assistance refugees Ethereum"; "ID2020 digital identity refugees Sovrin Hyperledger Indy World Bank ID4D"; "UNHCR Policy on the Protection of Personal Data of Persons of Concern biometric data refugees".

## Who else does something similar

| Project or paper | What it does | Source |
|---|---|---|
| ID2020 | Public-private partnership that sets identity standards and runs pilots for refugees and stateless people. A Mae La Camp (Thailand) pilot linked blockchain IDs to iris recognition; an Accenture/Microsoft prototype worked with UNHCR and was shown at the UN in 2017. Dated 2017-2018 coverage. (S/P mix, snippets) | [OECD OPSI, ID2020 Alliance](https://oecd-opsi.org/innovations/id2020-alliance/) |
| Hyperledger Indy / Sovrin | Self-sovereign identity ledger; its designers say it was built with refugees in mind. A Hyperledger identity wiki lists ID2020, UNHCR, Sovrin/Evernym and a Red Cross refugee-identity pilot. Current deployments not verified. (P, snippet) | [Hyperledger identity wiki](https://wiki.hyperledger.org/display/SISIG/IDENTITY) |
| UNHCR and WFP Building Blocks | Cash-transfer system on a permissioned Ethereum network (Parity, Proof-of-Authority) in Jordan camps. Refugees authenticate with an iris scan tied to UNHCR's biometric system. 2019 reporting: about 106,000 Syrian refugees redeeming transfers. Critics argued a conventional database could do the job. (P/S, snippets) | [ITU, WFP and blockchain (2020)](https://www.itu.int/hub/2020/04/how-the-world-food-programme-uses-blockchain-to-better-serve-refugees/); [Parity case note](https://www.parity.io/blog/parity-ethereum-to-aid-100-000-refugees) |
| World Bank ID4D | Identification for Development initiative. Its figure: 1.1 billion people lack any officially recognised identity. No blockchain system found in these results, and no formal link to ID2020 or Sovrin found. (P, snippet) | Search results only; primary World Bank page not fetched |
| UNHCR data protection policy (2015) | Governs personal data of persons of concern. Country Representatives are controllers; a Data Protection Officer sits at headquarters. A UNHCR Poland notice describes biometric deletion after a programme ended. The 2022 Protection and Privacy framework was not read. (P, snippet) | [CALP, policy text](https://www.calpnetwork.org/ar/publication/policy-on-the-protection-of-personal-data-of-persons-of-concern-to-unhcr/); [EJIL:Talk commentary](https://www.ejiltalk.org/data-protection-in-international-organizations-and-the-new-unhcr-data-protection-policy-light-at-the-end-of-the-tunnel/) |

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/identity/src/document.rs`: `DidDocument` with `rotate` and `revoke(height)` and `is_revoked(height)`. `crates/identity/src/did.rs`: `Did` parsing. [features.toml](../../features.toml) `self-sovereign-identity-records` (line 1913): class RESEARCH, status working, tests `crate:maya-identity`.
- `crates/smart-account/src/recovery.rs`: `GuardianSet { guardians, threshold, delay_blocks }`. A quorum's approval takes effect after `delay_blocks`, and any current owner key can cancel it. An owner who lost every key gets the account back. Doc comment at lines 1-8. Registry entry `smart-account` at [features.toml](../../features.toml) line 3095.
- `docs/identity.md`: describes `did:maya2c:<address>` and the 1,984-byte hybrid public key. `rg -i "biometric|recovery|guardian" docs/identity.md` returns nothing.
- Personal data and GDPR appear only as an open counsel question: [docs/legal/QUESTIONS_FOR_COUNSEL.md](../legal/QUESTIONS_FOR_COUNSEL.md) lines 44-45 ("does a commitment to personal data count as personal data under GDPR when the chain is immutable?").
- `rg -il "refugee|humanitarian|gdpr|nagoya|outer space|debris" docs crates hal` returns only CATALOG.md and QUESTIONS_FOR_COUNSEL.md. No refugee or humanitarian code exists.

## How a Maya2C version would differ

- **Recovery without papers or a seed phrase.** The account layer already has guardian recovery with a height-based delay and an owner cancel window (`recovery.rs`). A refugee identity would reuse it: a guardian set chosen by the person, with an aid body as one guardian. Design claim only, not built.
- **No biometric on chain.** Biometric enrollment, if ever done, stays with a partner that runs its own data protection regime. Only a commitment or a key hash goes on chain. This is the opposite of the Building Blocks design, which keeps the biometric link in the field.
- **Cost of the trade.** Recovery is delayed by design. A person in an emergency waits `delay_blocks`. Guardian quorums can also be coerced; the owner-cancel window protects only an owner who is still in control.

**Legal and ethical gates.** GDPR special-category rules for biometric data, and the erasure question for immutable records (counsel question, QUESTIONS_FOR_COUNSEL.md:44-45). UNHCR's own policy governs data it holds. No personal data on a public chain without a counsel opinion and a data protection impact assessment. Refugee-deployment partners are not chosen; do not name any.

**Verdict: similar exists (ID2020, Sovrin/Indy, Building Blocks). Maya2C differs at the design level: guardian recovery and height-based delay on the account, with no biometric on chain. Not built; a refugee deployment waits on an NGO partner and legal review.**

**forbidden**: "first refugee identity", "only portable refugee identity", "unprecedented identity recovery", "refugee identity on Maya2C is live".
