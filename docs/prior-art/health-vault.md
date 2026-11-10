# Prior art: patient-owned medical records granted per visit (health-vault)

**Search status:** searched 2026-11-10 (partial: two searches; MyData and official Kanta/THL sources not reached). Two searches. MyData (Finland)
and the official Kanta/THL sources were **not** reached; Apple's own developer
documentation was not read. Legal analysis was not attempted. This file
authorizes **no** "first", "only" or "unprecedented" claim.

## What exists today

| Project | What | Notes |
|---|---|---|
| SMART on FHIR | OAuth 2.0 apps authorized per patient and scope against EHRs; FHIR R4 `Consent` resource records choices | Standard in production; Cures Act export work (JAMIA 2024, PubMed 38287642) |
| Apple Health Records | Pulls records from providers over SMART on FHIR; user chooses data types and recipients | Per Healthcare IT News and ONC wiki; ONC wiki notes patient-driven consent fits emergencies poorly |
| Kanta / My Kanta (Finland) | National repository; patient manages consent and can see access logs | Only a clinic privacy page found, not an official source |
| MedRec (MIT Media Lab / BIDMC) | Ethereum smart contracts for patient control of record distribution | Research project, 2016-18 era |
| MyHealthData (Korea) | Blockchain MyData platform, 1,228 sign-ups in a KAIST study | Small pilot |

## How a Maya2C version would differ (verified in the tree)

- Chain rule: "No claim preimage reaches chain state, ever. Only commitments and roots"
  (`crates/identity/src/lib.rs:1-12`). Records would stay off chain; the chain
  would carry grants and revocations. MedRec-style designs place access rules on a
  ledger too, so ledger-held consent is not itself a difference.
- Per-visit grant and an access log are specified in the catalog only for the
  genetics case (`genetics-vault`, "consent and an access log, data off chain"),
  W4, and not in code.
- Credential model: `identity-app` is planned; `crates/identity` has the `did`,
  `attestation`, `document` modules and `RevocationPage`
  (`crates/identity/src/lib.rs:53-58`), the revocation mechanism a per-visit grant
  would reuse. Not tested for health use.
- Post-quantum signing (ML-DSA, SLH-DSA) of grants would matter for records read
  for decades; `crates/archive/src/seal.rs:1-30` shows SLH-DSA seals chosen for
  that reason. Providers and SMART/FHIR deployments were not checked for PQ.
- No FHIR, HL7 or consent-resource code exists in the tree.

## Legal and ethical gates

- Health-data law per country: HIPAA (US), GDPR special-category data (EU, with
  right to erasure conflicting with immutable ledgers), Finland's Kanta rules,
  Korea PIPA. Counsel review before any pilot (`docs/legal/QUESTIONS_FOR_COUNSEL.md`).
- A medical device or medical-records-vendor classification may apply.
- Emergency break-glass access must exist; patient-only consent can cost lives.
- Recovery when a patient loses keys; guardians for minors and incapacity.
- Records themselves stay with providers or patient-held encrypted stores, never on chain.

## What may be said publicly

- Accurate: "Maya2C has no health-record code today; per-visit access grants are a planned design."
- **forbidden**: "first patient-owned health records", "only blockchain medical records", "HIPAA-compliant", "unprecedented", "world's first".

## Verdict

Similar exists — differentiate by chain-anchored, post-quantum-signed grant and
revocation on top of off-chain records, not by patient control as such, which
SMART on FHIR, Kanta and MedRec already provide. Nothing is built.

## Sources (retrieved 2026-11-10)

- https://www.healthcareitnews.com/news/cerner-meditech-support-record-sharing-apple
- https://smarthealthit.org/author/awhite/page/2
- https://pubmed.ncbi.nlm.nih.gov/38287642/
- https://www.media.mit.edu/projects/medrec/overview/
- https://blockchain.ieee.org/technical-briefs/july-2018/medrec-patient-control-of-medical-record-distribution/
- https://koasas.kaist.ac.kr/bitstream/10203/287863/1/121748.pdf
- https://www.pihlajalinna.fi/en/general-information/processing-of-your-personal-data (clinic page, weak)

