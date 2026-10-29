# Plan: Overnight run 2026-10-18 02:40 → 09:00 KST

**Confirmed by Eric:** "do your max until morning 9 AM … finish all the work including build list".
**Complexity:** Large, so the work is split into lanes.

## Constraints
- **Cargo builds run in one lane only.** Parallel builds wedge `target/` or exhaust commit charge (`os error 1455`).
- **The live testnet is not touched;** the 7-day run must not halt. Key migration happens with Eric in the morning.
- **Nothing that costs money, nothing on mainnet, no economics.**
  - Sites deploy only through their existing pipelines: GitHub Pages runs on a merge to master, and Netlify keeps rollback.
- **No claims words.** No "first", "only", "unbreakable"; no invented numbers; "Maya2C" is one word.

## Lanes
| Lane | Who | Work | Ships as |
|---|---|---|---|
| R | me (serial cargo) | Sealed validator keys: Argon2id + ChaCha20-Poly1305, `--seal-validator-key`, plaintext refused under `production` | PR |
| R | me | Supply chain: SBOM + `cargo vet` init + build-provenance attestation on release | PR |
| R | me | Review and merge #116, #119 when CI is green; triage dependabot | merges |
| R | me | ADR for PQ level-5 defaults (ML-DSA-87 / ML-KEM-1024): consensus/wire change, needs a re-genesis, so the decision gets written down first | ADR PR |
| W1 | agent | maya2c.dev: land the 6 unpushed `feat/ui-presentation` commits, deeper 3D, social footer | PR |
| W2 | agent | eric.maya2c.dev: rebuild as a versioned 3D Astro site with the same content, deploy to Netlify | deploy + private repo |
| W3 | agent | GitHub profile README (EricWijesinghe/EricWijesinghe), Maya2C README presentation | repo + PR |

Socials, from the GitHub profile:
- GitHub https://github.com/EricWijesinghe
- LinkedIn https://www.linkedin.com/in/ericwijesinghe
- Facebook https://www.facebook.com/EricWijesinghe2002/
- Instagram https://www.instagram.com/ericwijesinghe2022/

## Validation
- Rust:
  - `cargo check -p maya2c-node -p maya-signer`
  - `cargo nextest run -p maya-signer -p maya2c-node`
  - `cargo deny check`
  - rust-reviewer and security-reviewer passes
- Web:
  - `npm run build`
  - CSP check
  - Lighthouse ≥ 95, `prefers-reduced-motion` respected
  - no external script origins

## Risks
| Risk | Mitigation |
|---|---|
| Sealed-key change breaks the live validators on restart | Plaintext stays accepted (with a warning) outside `production`; no live restart tonight |
| A heavy 3D bundle slows the site | three.js self-hosted and lazy-loaded, static fallback, Lighthouse gate |
| A Netlify deploy overwrites a git-linked build | Deploy is reversible: the previous deploy stays in Netlify for rollback |
