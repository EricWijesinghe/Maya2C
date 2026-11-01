# Runbook: maya2c.dev edge settings (Cloudflare) — Eric applies

Status: **pending, Eric applies.** Nothing here is applied by the repository or by CI.
Claude has no Cloudflare credential and must not hold one (threat model P2).

Why this exists: the 2026-10-06 injection came through the edge. The repository
controls the bytes the origin (GitHub Pages) serves. The edge can rewrite them
after that. The `site-integrity` workflow hashes what the edge serves and fails
on any difference, so the edge settings below must not change the bytes, except
for the two Cloudflare rewrites the check already normalises.

## 1. Turn off the rewrites that change bytes

Dashboard, zone `maya2c.dev`:

| Setting | Path | Set to | Why |
|---|---|---|---|
| Auto Minify (JavaScript, CSS, HTML) | Speed > Optimization > Content Optimization | **Off** | Rewrites the bytes the check compares. Every page would fail hourly. |
| Rocket Loader | Speed > Optimization > Content Optimization | **Off** | Rewrites script tags. Not a benign rewrite under the check's rules. |
| Email Address Obfuscation | Scrape Shield | On or off | Either works. On is normalised by `site-integrity` (decoded, not hidden). |

Web Analytics beacon: may stay on. The check removes only the exact beacon tag
(`data-cf-beacon=` on a `static.cloudflareinsights.com/beacon.min.js` source).

## 2. Response headers at the edge (defence in depth)

Pages cannot set headers. These go in a Transform Rule. Create it in
**Rules > Transform Rules > Modify Response Header**:

- Name: `maya2c security headers`
- Custom filter expression: `(http.host eq "maya2c.dev")`
- Set static, one row each:

| Header name | Value |
|---|---|
| `Content-Security-Policy` | `frame-ancestors 'none'; script-src-attr 'none'` |
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=(), payment=(), usb=()` |
| `Strict-Transport-Security` | `max-age=31536000` |

Do **not** add `includeSubDomains` to HSTS yet: the tunnel-served subdomains
(`explorer`, `status`, `faucet`, `chat`) have not all been checked for HTTPS-only.

The page's own CSP (a `<meta>` built by `docs/site/astro.config.mjs`) remains the
authoritative policy. The edge header adds `frame-ancestors` and `script-src-attr 'none'` on top of it.
Astro 7 rejects `script-src-attr` in its own config, so it can only be set here.
No built page has an inline event handler (`on*=` attribute; checked 2026-10-31,
zero in 86 HTML files). Load a page in a browser after applying and confirm the
console shows no `script-src-attr` violation.
An edge that is compromised can drop these headers; the meta CSP and SRI do not
depend on the edge, which is why they are the controls that matter.

## 3. Verify

```
curl -sI https://maya2c.dev/ | rg -i "content-security|x-content-type|referrer-policy|permissions-policy|strict-transport"
```

Expect all five headers. Then wait for the next hourly `site-integrity` run
(`:41` past the hour). A green run after a change proves the change did not
alter any served HTML, JS or CSS byte.

## 4. Token scope

The `site-integrity` job holds no Cloudflare secret, by design. Keep it that way:
do not add a Cloudflare token to the repository or to any workflow. Tokens used
by the deploy path should be zone-scoped (Zone: Read, DNS: Edit only if DNS is
managed there), not account-admin. This is owner-attested (2FA and token
rotation are recorded in `notice-draft.md`), not checked by CI.

## 5. Known limits of the check (so a green run is read correctly)

- Only files in `integrity-manifest.json` are compared: every HTML, JS, MJS and CSS
  file in the build. A new page path that the build did not produce is **not**
  detected. Sitemaps, `robots.txt`, feeds and `.txt` files are not covered.
- The check runs hourly. A script served to a visitor between two runs is not
  caught by the check; SRI and the CSP are the per-request controls.
- For about an hour after each deploy the check skips (CDN propagation,
  2026-10-13). A clean run after a skip says nothing about that hour.
- Dynamic `import()` chunks get no SRI (the browser cannot attach one). They are
  in the manifest, so the hourly check covers them.
- The PC-side watcher (`D:\Maya2C-incident\integrity-watch.ps1`) keeps running as a
  second witness. Retiring it is Eric's decision, after this check has run clean
  for two weeks.

## Rollback

Disable the Transform Rule (Rules > Transform Rules). Leave Auto Minify and
Rocket Loader off: while either is on, the hourly check fails by design.
