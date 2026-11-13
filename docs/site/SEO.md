# Search and link-preview notes for maya2c.dev

What the site already does, what this pass added, what a build enforces, and
what only the owner can do. Nothing here has been submitted to a search engine
or any external service.

## What the build enforces

`npm run build` ends with `scripts/check-seo.mjs`, which reads `dist/` and
fails on:

- a missing or empty `<title>`, or two pages with the same title
- a missing or empty meta description (pages marked `noindex` are exempt)
- a canonical link that is absent or does not equal the page's own URL
- JSON-LD that does not parse
- an internal link or `<img>`/`<script>`/`<link>` target that does not exist in the build
- not exactly one `<h1>` per indexable page, or a heading level skipped (h1 then h3)
- an `<img>` without an `alt` attribute
- a missing `og:image`, or one that is not served from maya2c.dev and present in the build

The existing workflow (`.github/workflows/site.yml`) runs `npm run build` in its
`build` job, so a failing check blocks the deploy. `scripts/check-csp.mjs` and
`scripts/check-fields.mjs` run in the same chain.

Negative test (run once, output kept in the PR): a hand-made page with a
duplicate title, two h1s, a broken link, an image without alt, invalid JSON-LD,
a missing canonical and an unserved og:image makes the checker exit 1 with all
eight problems listed.

## What is in place

| Item | Where |
|---|---|
| Per-page title and description | Starlight frontmatter; `scripts/ingest.mjs` derives them for `docs/*.md`, with `SEARCH_TITLES` overrides |
| Canonical URLs | Starlight, checked by `check-seo.mjs` |
| robots.txt | `public/robots.txt`, points at the sitemap |
| Sitemap | `@astrojs/sitemap` (included by Starlight): `/sitemap-index.xml` and `/sitemap-0.xml`, 91 URLs. `/sitemap.xml` does not exist; robots.txt and the IndexNow step use the index |
| hreflang | Not applicable: the site is English only, no i18n configured |
| Open Graph and Twitter cards | `src/components/Head.astro` (per-page image, title, description) and Starlight |
| Per-page link-preview images | `scripts/gen-og.mjs` renders 18 cards from `src/data/og-pages.json` into `public/og/pages/`. Pages not listed fall back to the section card |
| Organization, WebSite, SoftwareApplication, SoftwareSourceCode | Site-wide graph in `astro.config.mjs` |
| BreadcrumbList and TechArticle | `src/components/Head.astro`, per docs page |
| SearchAction | Not added. The site search is Pagefind, which opens a modal and has no URL template to point a SearchAction at |
| Wallet SoftwareApplication | Not added. The site has no wallet download to describe; the node has one |
| CSP | Unchanged. JSON-LD blocks are `application/ld+json`, which browsers do not execute, so the CSP does not block them and needs no hash. `check-csp.mjs` still passes |
| security.txt | `public/.well-known/security.txt`: Contact, Expires, Preferred-Languages, Canonical, Policy |
| Font loading | Inter (body) preloaded from the self-hosted fontsource file. Fontsource CSS uses `font-display: swap`. KaTeX math fonts use `block` (KaTeX's own CSS; math pages only) |
| 404 | Starlight's `404.html`, served by GitHub Pages; it is not indexed and has no canonical by design |
| Headings | One h1 per page, no skipped levels. The maintenance dialog's heading is an h2 now. Field cards on `/fields/` are h2 |
| Images | Header emblem has width, height and `alt=""` (decorative; the link carries the name). Badges on the contributors page carry alt text |

## Keyword themes

Taken from the pages' own titles and summaries. Use these words where they are
true of the page; do not add a term a page does not support.

- post-quantum blockchain, post-quantum layer-1 blockchain
- ML-DSA-65 (FIPS 204) and SLH-DSA (FIPS 205) signatures, hybrid transaction signatures
- ML-KEM (FIPS 203) in the peer transport
- quantum-safe or post-quantum wallet, signing post-quantum transactions
- DAG-BFT finality, validators, GPU mining on a testnet
- post-quantum security comparison with other chains (dated, sourced)

Brand: "Maya2C" is one word, in titles, copy and alt text.

## Owner checklist (manual; these are external services)

Nothing below was done by the build or by Claude. Each step publishes or
registers something with a third party, so each one is yours.

1. Google Search Console: add the maya2c.dev domain property (DNS TXT
   verification), then submit `https://maya2c.dev/sitemap-index.xml`.
2. Bing Webmaster Tools: the `msvalidate.01` meta tag is already in the head
   (`astro.config.mjs`). Verify, then submit the same sitemap. IndexNow already
   pings on each deploy (`.github/workflows/site.yml`), using the key file in `public/`.
3. Link-preview debuggers (X card validator, Facebook Sharing Debugger,
   LinkedIn Post Inspector): paste `https://maya2c.dev/guides/quickstart/` and
   one other page to confirm the per-page card appears.
4. Google Rich Results Test on the home page (Organization, WebSite,
   SoftwareApplication) and one guide (BreadcrumbList, TechArticle). Expect
   warnings for fields Google does not require; errors are the ones to fix.
5. Keep `softwareVersion` in `astro.config.mjs` equal to the newest release tag
   when a release is cut, and `expires` in `security.txt` renewed before 2027-11-08.
6. Core Web Vitals: run Lighthouse or PageSpeed Insights on the live site after
   a deploy. No measurement is recorded in this pass.

## Regenerating link-preview images

    npm run build            # generates only missing cards
    node scripts/gen-og.mjs --force   # re-render all 18

Cards use the system sans-serif font (Segoe UI where present), not the brand
face, because librsvg cannot load the self-hosted woff2 files. The PNGs are
committed; the build does not re-render them unless they are missing.

## Known gaps

- No Lighthouse or Core Web Vitals run in this pass.
- The Starlight `i18n` collection warning in the build log predates this pass
  and is harmless (Starlight looks for translations and finds none).
- Badge images on `/guides/contributors/` have no width and height attributes;
  they come from authored markdown and are small SVGs.
- Below-the-fold images are not lazy-loaded through a site-wide rule; content
  images are few and small.
