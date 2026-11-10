# Prior art: Fabrication rights (licensed, traceable files for 3D printers)

**Search status:** searched 2026-11-10 (web search; sources cited below).

Feature: CATALOG §I row `fabrication-rights` ([docs/ecosystem/CATALOG.md](../ecosystem/CATALOG.md)). Not built. R0 idea, provenance only.

**Query:** "3D printing file licensing DRM blockchain digital rights print license".

## Who else does something similar

| Work | What it does | Source |
|---|---|---|
| US patent 10,513,077 | A 3D printer checks a blockchain-stored license before printing and refuses if authentication fails; the licensor can cap the number of licenses. Patent, not confirmed in commercial use. | [USPTO PDF](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/10513077) |
| US patent 10,063,529 | Secure sharing of 3D models through a distributed ledger, with decryption keys released under use restrictions. | [USPTO PDF](https://image-ppubs.uspto.gov/dirsearch-public/print/downloadPdf/10063529) |
| University of Exeter | Watermarking of printed objects, linked to a blockchain licence record; a platform for licensing printable products. Launch status not verified. | [Exeter news](https://news.exeter.ac.uk/?p=3605), [3D Printing Industry](https://3dprintingindustry.com/news/researchers-combine-digital-watermarking-and-blockchain-to-protect-3d-printing-ip-208176/) |
| Academic licensing model | Worked example: a smart contract records the licence and the printer checks it before printing; licences resold as tokens. | [3DPrint.com](https://3dprint.com/224165/blockchain-3d-printed-ip/) |

The search found no confirmed commercial deployment of print-blocking licences. Trade press notes scepticism about the benefit.

## What Maya2C has (checked in repo, 2026-11-10)

- `crates/rwa` exists. No `notary` or `render-market` crate exists in `crates/`; the catalog row names both as builds-on and they are unbuilt.
- No print or licence-check code anywhere in the repo.

## How a Maya2C version would differ

- **Provenance, not blocking.** The catalog states the rule: provenance only, never lock-in. The patents above refuse to print; a Maya2C version would record who licensed and made a file, and would not stop a printer. That is a policy choice, not yet code.
- **Records on the chain, files off it.** Only a hash and licence terms would go on chain, consistent with the catalog's no-lock-in rule. Not built.

**Verdict: similar exists (two US patents, an academic model, an Exeter platform). Differentiate by provenance-only, non-blocking licence records. Patent freedom-to-operate is not assessed here; that needs a lawyer, not this record.**

**forbidden**: "first 3D print licensing on chain", "only blockchain print DRM", "prints can be blocked by Maya2C".
