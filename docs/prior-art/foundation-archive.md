# Prior art: civilisation's knowledge kept verifiable for centuries (foundation-archive)

**Search status:** searched 2026-11-10 (partial: two searches; Arweave yellow paper not read). Two searches. Rosetta Project,
Software Heritage and Internet Archive internals were reached only through
secondary or tangential results; Arweave's yellow paper was not read. This file
authorizes **no** "first", "only" or "unprecedented" claim.

## What exists today

| Project | Mechanism | Durability claim and caveat |
|---|---|---|
| Arweave | Pay once; part of the fee funds an endowment | "At least 200 years" depends on the endowment keeping value and storage cost falling; history since 2017 is short for that horizon |
| Internet Archive | Institutional web and media archive | Arweave announced (2020-07-15) a partnership to store Wayback metadata; possibly stale |
| GitHub Archive Program (Arctic World Archive, Svalbard) | Piql film; partners include Long Now, Internet Archive, Software Heritage, Microsoft Research, Bodleian, Stanford | Piql states about 1,000 years for film; vendor claim |
| Microsoft Project Silica | Quartz glass storage | Over 10,000 years per one secondary source; not verified |
| Long Now Rosetta Project | Micro-etched nickel disk of about 1,500 languages | One source only |
| IPFS / CAR | Content-addressed storage | Not a durability guarantee by itself |

## How a Maya2C version would differ (verified in the tree)

- Content addressing and read-back from any host exist: `crates/archive/src/lib.rs:1-12`
  (CAR v1 batches of historical blocks, CID manifest, readable from a directory,
  IPFS node or Arweave gateway; Arweave read-only, `crates/archive/src/arweave.rs:1-3`).
  This is reuse, not a difference.
- Signed provenance meant for decades: forward-secure SLH-DSA-SHAKE-256f seals over
  archive roots, chain of epoch transitions, FIPS 205 category 5
  (`crates/archive/src/seal.rs:1-30`); algorithm ageing handled by
  `docs/resealing.md` and `crates/archive/src/lib.rs:34` (`pub mod reseal`). This
  is the concrete candidate difference: signatures that survive hash and signature
  algorithm change. Whether Arweave, Software Heritage or Piql offer comparable
  resealing: **not searched**.
- Data availability sampling over Reed-Solomon squares: `crates/da/src/lib.rs:1-12`.
- Scope today: archives chain history, not "knowledge". `foundation-archive`,
  `long-archive` and `research-records` are catalog rows, planned, not code. No
  physical-media (film, glass, DNA) code exists.
- No endowment or storage-payment mechanism for archives was found.

## Legal and ethical gates

- Copyright and licence for each corpus; right-to-be-forgotten requests (GDPR)
  conflict with permanence; takedown process needed.
- Cultural and indigenous knowledge: consent and stewardship before ingest.
- Selection bias: publish the criteria for what is "foundational".
- Open, documented archival formats; migrate hashes through the suite registry.
- An archive promise longer than its funding is a claim nobody can keep; state the
  horizon actually funded.

## What may be said publicly

- Accurate: "Maya2C's archive crate writes content-addressed archives of chain history with post-quantum seals; it does not yet archive general knowledge."
- **forbidden**: "first archive for civilisation", "only chain that keeps knowledge forever", "lasts forever", "unprecedented", "world's first".

## Verdict

Similar exists — differentiate by forward-secure, resealable post-quantum signatures
over content-addressed archives (`crates/archive`), pending a search of how others
handle algorithm ageing. No durability or "forever" claim is supportable.

## Sources (retrieved 2026-11-10)

- https://ar.io/articles/what-is-arweave/
- https://ardrive.io/can-data-really-be-stored-forever
- https://permaweb-journal.ar.io/article/storage-endowment-explained.html
- https://www.theblock.co/post/71398/theblockcrypto.com
- https://longnow.org/ideas/long-now-partners-with-github-on-its-long-term-archive-program-for-open-source-code/
- https://www.datacenterdynamics.com/en/news/arctic-world-archive-adds-latest-data-deposit-in-svalbard-facility
- https://archiveprogram.github.com/partners/

