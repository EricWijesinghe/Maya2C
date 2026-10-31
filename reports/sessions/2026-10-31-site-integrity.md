# Session 2026-10-31: site integrity (branch ci/site-integrity)

Goal: close the CDN and site.yml gaps in THREAT_MODEL_SIGNER_CHAIN.md (P2, P8): build without deploy rights, a signed manifest of what is built, an hourly live comparison that keeps the differing bodies, SRI, and the edge settings written down for Eric.

## Done

1. `.github/workflows/site.yml` split into `build` (contents read, `persist-credentials: false`, `npm ci --ignore-scripts`), `attest` (id-token and attestations write; signs the manifest), `deploy` (needs both). Actions pinned to full SHAs (reused from the repo's existing pins).
2. `xtask site-integrity` (`xtask/src/site_integrity.rs`): `sri`, `manifest`, `check`. Hashes every served HTML, JS, MJS and CSS file. HTML is hashed after removing only Cloudflare's own rewrites (decoded email links and text, exact beacon and email-decode scripts).
3. `.github/workflows/site-integrity.yml`: hourly (`:41`). Takes the latest successful `site` deploy's manifest, verifies its attestation, fetches every listed file from maya2c.dev, runs `check`. On difference: uploads differing bodies, opens or updates a `site-integrity` issue, fails the run. Skips the first 60 min after a deploy (CDN lag).
4. SRI: `sri` adds `integrity="sha384-..."` to same-origin script and stylesheet tags in every built page. CSP: the existing Astro CSP already hashes inline scripts and was not changed. Tried adding `script-src-attr 'none'` to Astro; Astro 7 rejects it, so it moved to the edge header in the runbook.
5. `docs/runbooks/site-edge-settings.md`: exact Cloudflare settings (Auto Minify off, Rocket Loader off, header Transform Rule). Marked **Eric applies**.
6. `docs/adr/ADR-071-site-integrity.md`.

## Verification (real output)

YAML and workflow lint:

```
$ actionlint .github/workflows/site.yml .github/workflows/site-integrity.yml; echo "actionlint rc=$?"
actionlint rc=0
$ python -c "yaml.safe_load(...)" on both files
python yaml: both parse
```

Every `uses:` line is pinned to a 40-hex SHA (grep check: no unpinned match).

Unit tests (`cargo test -p xtask site_integrity`, CARGO_BUILD_JOBS=2):

```
test site_integrity::tests::base64_standard_vectors ... ok
test site_integrity::tests::malformed_cloudflare_hex_is_not_decoded ... ok
test site_integrity::tests::cloudflare_email_and_beacon_normalize_to_built_page ... ok
test site_integrity::tests::sri_fails_on_missing_referenced_file ... ok
test site_integrity::tests::injected_script_is_not_normalized_away ... ok
test site_integrity::tests::email_obfuscated_live_page_passes_check ... ok
test site_integrity::tests::sri_adds_hash_once_and_skips_external_and_inline ... ok
test site_integrity::tests::changed_js_and_missing_file_are_counted ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 31 filtered out
```

Clippy on xtask: zero warnings in `site_integrity.rs` after fixes (`unwrap_used` is denied by the workspace; replaced with `expect`).

Site build, without install scripts (`npm ci --ignore-scripts` exit 0):

```
check-csp: 85 pages, every inline script hashed
[build] 85 page(s) built in 8.87s
[build] Complete!
build rc=0
```

Pipeline on the real build output (local stand-in for the workflow):

```
site-integrity: SRI added to 85 HTML page(s) of 117
site-integrity: commit fa89f36cb4b1f34236392dda2ae0569cf1523f17 checked 117 files, 0 mismatch(es)     (dist as live)
site-integrity: commit fa89f36c... checked 117 files, 0 mismatch(es)     (live copy with Cloudflare-style email + beacon rewrites on 404.html)
```

Injection on a live copy (`<script src="https://evil.example/x.js">` added to 404.html, guides/testnet page removed):

```
MISMATCH 404.html expected=f89faa80... got=c080fdf7...
MISSING  guides/testnet/index.html
site-integrity: ... checked 117 files, 2 mismatch(es)
xtask: 2 served file(s) differ from the deployed build
rc=1
```

Built HTML carries SRI, e.g. `<script type="module" src="/_astro/page.cNngjUgL.js" integrity="sha384-eK/k...">`.

Repo hygiene: the build rewrites `docs/site/public/health.json` and `progress.json`; restored with `git checkout` so the PR carries only the intended files.

## NOT VERIFIED: needs a run on GitHub

- `site.yml` build on a fresh runner: the `cargo run -p xtask` step uses the pinned nightly from `rust-toolchain.toml`, which rustup must install on the runner. Untested there.
- `attest` job and `gh attestation verify --signer-workflow` in `site-integrity.yml`: flags used per gh 2.102 on this PC, not run in Actions.
- Live fetch of maya2c.dev from a GitHub runner: Cloudflare may challenge a runner User-Agent. The UA is the same Chrome string the PC watcher uses.
- Cloudflare's exact rewrite form for the `data-cfemail` span and the beacon `src` suffix is taken from the watcher's regexes and public Cloudflare behaviour, not from a captured live response. The first hourly run is the test.
- Browser-side SRI and CSP behaviour: not loaded in a browser. The HTML contains the attributes; enforcement not observed.
- `script-src-attr 'none'` at the edge: not applied; the runbook asks for a browser console check.

## What Eric must apply

See `docs/runbooks/site-edge-settings.md`: Auto Minify off, Rocket Loader off, the response-header Transform Rule. Nothing in this PR touches Cloudflare or DNS.

## Gaps left open

- Files outside the manifest (sitemaps, `robots.txt`, new unbuilt paths) are not compared.
- The check is hourly; per-request protection is SRI and CSP only.
- `style-src 'unsafe-inline'` remains (inline style attributes); not removable without a style audit.
- The PC watcher was not changed; it is the second witness until the CI check has run clean for two weeks.
- No `npm` lifecycle script is allow-listed; none was needed in the local build.
