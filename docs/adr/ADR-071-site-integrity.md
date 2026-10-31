# ADR-071: the site build cannot deploy, and the live site is checked against a signed manifest

- Status: Accepted (2026-10-31). Implemented on `ci/site-integrity`; the live check has not run on GitHub yet.
- Scope: `.github/workflows/site.yml`, `.github/workflows/site-integrity.yml`, `xtask site-integrity`, `docs/site/astro.config.mjs` (CSP), `docs/runbooks/site-edge-settings.md`.

## Context

On 2026-10-06 a stolen Cloudflare token injected a script into every page of maya2c.dev. The only watch ran on one PC and compared byte counts. It kept no copy of a differing page, so a 5558-byte difference on 2026-10-13 could not be explained after the fact. The threat model (`docs/security/THREAT_MODEL_SIGNER_CHAIN.md`, P2 and P8) named three gaps: the site build job held `id-token: write` and ran `npm ci` with lifecycle scripts; no CI job compared the built output with what the edge serves; no subresource integrity was set.

## Decision

1. **Split the build from the deploy.** `build` has `contents: read` only, `persist-credentials: false`, and runs `npm ci --ignore-scripts`. The site builds without install scripts (checked locally with the committed lockfile). A package that later needs one is allow-listed by name in `site.yml`, with the reason.
2. **Sign a manifest of what was built.** `xtask site-integrity manifest` writes SHA-256 for every served HTML, JS, MJS and CSS file. HTML is hashed after removing only Cloudflare's own rewrites (email obfuscation, the Web Analytics beacon, the email-decode script). `attest` signs the manifest with actions/attest-build-provenance. `deploy` waits for `attest`.
3. **Check the live site hourly from a runner with no Cloudflare credential.** `site-integrity.yml` takes the manifest of the latest successful deploy, verifies its attestation, fetches each listed file from maya2c.dev, and compares. A difference uploads the differing bodies, opens or updates a `site-integrity` issue, and fails the run. The check skips the first hour after a deploy, when CDN lag produces differences that are not attacks.
4. **SRI on every same-origin script and stylesheet** in the built HTML (`xtask site-integrity sri`). The browser refuses an altered file even when nobody runs the check.
5. **Edge settings are not in the repository.** Headers and the minify and rewrite switches go in `docs/runbooks/site-edge-settings.md`, marked as applied by Eric.

## Alternatives rejected

- **A shell or Node compare script.** The repository prefers Rust tooling, and the normalisation rules are the part that must be tested. They are unit-tested in `xtask/src/site_integrity.rs`.
- **Attesting inside the build job.** Needs `id-token: write` in the job that runs npm. Rejected; the attestation has its own job.
- **Trusting a manifest fetched from maya2c.dev.** The attacker controls that origin. The manifest comes from the Actions artifact of a verified run.
- **Adding `script-src-attr 'none'` to the Astro CSP.** Astro 7 rejects it in `security.csp.directives`. It goes in the edge header in the runbook instead.

## Consequences

- The check covers the manifest's files only. A new path the build did not produce is not detected. Sitemaps and `robots.txt` are not covered.
- An injected script served to a visitor between two hourly runs is not caught by the check. SRI and the page CSP are the per-request controls.
- Auto Minify and Rocket Loader must stay off at Cloudflare, or the hourly check fails by design.
- The PC-side watcher keeps running as a second witness until the CI check has been clean for two weeks.
