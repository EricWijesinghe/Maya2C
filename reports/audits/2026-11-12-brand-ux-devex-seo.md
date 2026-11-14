# Brand, UX, DevEx, SEO and trust audit, 2026-11-12

Read-only audit of every public surface, plus a plan. Nothing was built, run
or deployed. Code evidence is `file:line` at commit `c7ebb4d2` (origin/master
when read). Web evidence was fetched on 2026-11-13 and is cited with its URL.

**What this audit does not repeat.** Visual design (hierarchy, contrast, type,
spacing, motion, per-surface scores) is already covered by
`docs/design/audit-2026-11.md` (U0, PR #478) and
`docs/site/AUDIT-2026-11-10.md`. This report covers what those leave out:
whether public statements agree with each other and with the tree, whether
the journeys work today, SEO, developer onboarding, trust, growth and GitHub.
Where a finding overlaps U0 it is cross-referenced, not restated.

**Not verified**, and why:

- No page was rendered in a browser. Findings about what a visitor sees are
  read from source, not from a screenshot.
- No Lighthouse or Core Web Vitals run exists (`docs/site/SEO.md:98`). No
  performance figure appears here.
- solana.com and the Aptos landing page answered HTTP 429 on every attempt
  (four URLs, 2026-11-13). Their cells say NOT FETCHED.
- Competitor pages were read through a text extractor. "Not observed" means
  the extractor did not return it, not that the site lacks it. Structured
  data on competitor sites was not measurable this way.
- The body of `scripts/join-testnet.sh` / `.ps1` was not audited.
- Branch `feat/ui-u3-website` is not on the remote (`git branch -r`,
  2026-11-13), so U3 conflicts are inferred from its stated scope.

## Severity rubric

| Level | Test |
|---|---|
| CRITICAL | A public statement is false today, or two public statements about security, audit or launch status contradict each other; or the item creates legal exposure. |
| HIGH | A primary journey (visit, install, run, contribute) breaks today, or a stale or wrong figure is shown as current. |
| MEDIUM | The journey completes, with friction or an inconsistency a careful reader will notice. |
| LOW | Polish, or a guard against a future mistake. |

## 1. Weakness register

### CRITICAL

| ID | Surface | Evidence | User impact | Fix |
|---|---|---|---|---|
| W-01 | README vs site: audit before mainnet | `README.md:298-299` says mainnet is blocked until the cryptography is independently audited; `README.md:80-84` says "Mainnet is deliberately blocked". `docs/site/authored/faq.md:92` says mainnet is to launch labelled "external audit pending"; `docs/mainnet-v1-plan.md:5-6` says mainnet comes before the audit money. | Two official pages give opposite answers to the question a cautious reader asks before anything else. Whichever one they read second makes the other look dishonest. | One sentence, owned by `docs/mainnet-v1-plan.md`, copied into README and FAQ: what launches unaudited, what stays off until audited, and the label the network carries. Add the pair to `cargo xtask claims-check` or `guides-check` so they cannot drift again. |
| W-02 | GitHub repository description | `gh repo view --json description` (2026-11-13): "Public testnet live at maya2c.dev." `docs/site/public/llms.txt:3`: testnet paused, stopped 2026-10-28. `curl https://status.maya2c.dev/api/status` and `https://explorer.maya2c.dev/` both returned HTTP 530 on 2026-11-13. | The one line shown in GitHub search, link previews and the repo header is false today. | Owner action: `gh repo edit --description` with the paused wording from `llms.txt`. Drop the `testnet` topic or keep it; add `post-quantum`. Put the description in the release checklist so it changes with network state. |

### HIGH

| ID | Surface | Evidence | User impact | Fix |
|---|---|---|---|---|
| W-03 | Site header: two dead primary links | `docs/site/src/components/Header.astro:19-20` link Explorer and Status; both hosts returned HTTP 530 (2026-11-13). | Two of six primary nav items land on a Cloudflare error page. The status page is unreachable exactly when someone checks whether the network is down. | While `TESTNET_PAUSED` is true, point both at static pages the site already builds: `/health/` (reads `public/health/incidents.json`) and `/stats/`. Longer term the status page should not depend on the machine it reports on: publish a static snapshot of the last `/api/status` with the site. |
| W-04 | Home, first screen while paused | `Hero3D.astro:21` chip starts at "connecting…", `:95` sets "offline"; `:33-36` four tiles default to "—". `Maintenance.astro:82-83` says "The home page says the testnet is paused", but `home.mdx` contains no such sentence (`grep -c paused` = 0). | A deliberate, documented stop reads as an outage. The strongest evidence the project has (7 d 15 h, zero halts, `README.md:43-46`) is absent from the first screen. | A third chip state driven by the same flag: "paused on purpose since 2026-10-28" with the measured run from `incidents.json`, linking to the stop report. Tiles show the last recorded values with their date, not dashes. Conflicts with U3; hand this to that lane as a requirement. |
| W-05 | SDK install commands fail | `sdks/sdk-js/README.md:6` `npm install maya2c.js`: `registry.npmjs.org/maya2c.js` returned 404 (2026-11-13). `sdks/sdk-ffi/bindings/python/README.md:6` `pip install maya2c`: `pypi.org/pypi/maya2c/json` returned 404 (2026-11-13). | A developer's first command fails. Most will not try a second. | Until publishing is approved (`npm publish` / PyPI need `APPROVED`), each README says "not published yet" and gives the from-source command. After publishing, a CI step resolves every install line in `sdks/*/README.md` against its registry. |
| W-06 | README badge row | `README.md:17` is a dynamic badge reading `status.maya2c.dev/api/status`, which returns 530. | The first badge on the repo page renders as an error while the network is paused. | Replace with a static badge built from a committed file (the pattern `docs/site/public/badges/` already uses), or remove it until the next genesis. |
| W-07 | README counts and crate table are stale | `README.md:93-102`: 154 subsystems, 61 verified, 72 working, 21 planned. `docs/site/public/health.json` (generated 2026-11-11): 166 total, 61 verified, 78 working, 24 planned. `CLAUDE.md` says 147 subsystems. `README.md:133` names the node `(root) custom-l1-node`; ADR-001 puts it in `crates/node`. `README.md:134` says "All `u64`" money math; ADR-046 moved amounts to 256 bits. | Three documents, three counts. The README states "a plain read of the file at this commit" (`:106`), which is no longer true. | Generate the block from `features.toml` with an xtask that has `--check`, the way `brand-assets` and `og-card` already work. Fix the two crate rows by hand. |
| W-08 | Transport parameter stated two ways | ML-KEM-768: `home.mdx:50`, `StackDiagram.astro:9`, `README.md:78`, `README.md:124`. ML-KEM-1024 preferred with 768 fallback: `llms.txt:3`, `docs/prior-art/pq-l1-comparison-2026-11.md` ("Maya2C side" table), `health.json` subsystem list. `home.mdx:50` also links to `/reference/hybrid-signatures/` (signatures), not `/reference/pq-transport/`. | A security parameter that differs between the homepage and the reference is the kind of detail a reviewer checks first. | One wording everywhere: "ML-KEM-1024, with ML-KEM-768 as fallback, layered on Noise". Fix the link. |
| W-09 | Stale test count; two gate totals | `health.json`: `tests.as_of` 2026-09-26 with the note "Replaced by a fresh count when this runs in CI"; no workflow writes it (open since `AUDIT-2026-11-10.md` item 8). `health.json` gates 6 of 8; `progress.json` gates 6 of 10. | A seven-week-old number on the homepage facts band, and two answers to "how many gates". | CI writes the count from the nightly run, or the fact is dropped. Delete the eight-gate field from `gen-health.mjs`; the ten-gate plan is the authority. |
| W-10 | Contribution path dead-ends | `README.md:266` links the "good first issue" label: 0 open issues carry it, 1 issue is open in total (`gh issue list`, 2026-11-13). `CONTRIBUTING.md:22-23`: accepted suggestions are mostly re-implemented, not merged. | A willing contributor finds nothing to pick up and is told their code will probably not be merged. | Keep the policy; change the offer to match it. Open 5 to 10 real, small, labelled issues (docs, tests, translations of guides), and say plainly in CONTRIBUTING what does get merged as written and how credit is given (`/guides/contributors/` already exists). |

### MEDIUM

| ID | Surface | Evidence | User impact | Fix |
|---|---|---|---|---|
| W-11 | One-command devnet is hidden | `cargo xtask kit up` (ADR-090, `docs/ecosystem/testnet-kit.md:8-11`) appears in no site guide and not in the README, which documents the older `cargo xtask up` (`README.md:210`). `quickstart.mdx:9` states about fifteen minutes; `:73-102` has the reader hand-write a genesis file. | The shortest route to a running chain exists and nobody is shown it. | Quickstart opens with the kit path; the hand-built path becomes "Do it by hand". README quickstart names one command. |
| W-12 | Python and Rust SDK packaging | `sdks/python/` has no `pyproject.toml` and its README has no install line; `sdks/rust/` has no README. | Two of the listed SDKs cannot be installed by a documented command. | Add the packaging file and a README with one install line each, marked unpublished until approved. |
| W-13 | Structured data gaps | `Head.astro:33-43` emits `TechArticle` on every non-splash page with no `dateModified` and no `author`. `/guides/faq/` gets no `FAQPage`. `astro.config.mjs:108` `sameAs` has one entry. | Search results cannot show question snippets or a freshness date. | `FAQPage` generated from the `##` questions in `faq.md`; `dateModified` from git; extend `check-seo.mjs` to require both. |
| W-14 | Explorer has almost no head metadata | `apps/explorer/src/ui.rs:85` `<html>` without `lang`; no meta description, no canonical; `:111` constant `og:title`; `:112,114` relative `og:image`/`twitter:image` (crawlers need absolute URLs). No `robots.txt` or sitemap route (`server.rs:102-123`). | Block, transaction and validator pages are the largest set of indexable URLs the project will have, and none can produce a correct link preview. | Absolute image URL from config, per-page description, canonical, `lang="en"`, `robots.txt`. Conflicts: #488, #484, #414, #410 all edit `apps/explorer/src`. |
| W-15 | Network name hard-coded | `apps/explorer/src/ui.rs:377`; `apps/status/src/page.html:76,117-122`; `apps/status/src/live.rs:472`; `apps/status/src/main.rs:231,241,417,476`. | At the next genesis every surface still says `maya-testnet-1` until each literal is found and edited. | Read the chain id from the node the service is already connected to. |
| W-16 | Product names follow two patterns | "Maya Wallet" (`apps/wallet-gui/ui/index.html:6`), "Maya Chat" (`apps/chat-gui/ui/index.html:6,16`), "Maya2C Hub" (`apps/hub/ui/index.html:6`). `BRAND.md:10-11` protects "Maya2C" and "MAYA" only. | The family does not read as one family, and "Maya Wallet" is not a protected name. | Decide one pattern, record it in `BRAND.md`, apply it in the U-lane for each app. Owner decision. |
| W-17 | Three theme colours | `astro.config.mjs:91` `#050b0f`; `ui.rs:107` `#030716`; `wallet-gui/ui/index.html:19` `#ffffff`. | Browser chrome differs per surface. | Covered by #488 (tokens). Listed so the token PR checks these three literals. |
| W-18 | Chat address format | `apps/chat-gui/ui/index.html:34` placeholder "Their address (64 hex)"; ADR-081 moves address text to Bech32m (#410, #434). | Two address formats across wallet and chat. | Accept both, show Bech32m. Belongs with #495 or the chat U-lane. |
| W-19 | Flat sidebar | `astro.config.mjs:175-196`: 20 guides in one list (open since `AUDIT-2026-11-10.md` item 16). | No audience grouping; "Run a validator" sits between "Mining" and "Break it". | Four groups: Start, Learn, Build, Run. Conflicts with #474, #490, #433, #486 (all edit `astro.config.mjs`). |
| W-20 | Absolute statements without a source | `QuantumFacts.astro:27`: a classical chain's signature bytes, "none of it survives a quantum computer". `:36`: memory-hard mining "keeps the network out of reach of cheap throwaway ASIC farms". | Both are stronger than the evidence in the tree. The second is a prediction about hardware economics with no measurement behind it. | Reword to what is known: Shor's algorithm breaks ECDSA and EdDSA given a large fault-tolerant machine (cite NIST IR 8547); memory-hardness "raises the cost of specialised hardware", no claim of outcome. |
| W-21 | Chat listed as built, without its class | `docs/site/src/data/ecosystem-apps.ts:22`: status "built", "Download it from the releases page". The prior-art record notes `maya-chat` is class RESEARCH, "unaudited; no groups, no metadata privacy" (`docs/prior-art/pq-l1-comparison-2026-11.md`, "Maya2C side" table). | A messenger download offered without the unaudited label the ledger carries. | Add the ledger's class and caveat to the card; generate the card status from `features.toml`. |
| W-22 | Platform list stated three ways | `home.mdx:75` "Windows, macOS, Linux"; `astro.config.mjs:118` `operatingSystem: "Linux, Windows"`; `AUDIT-2026-11-10.md` item 15 found darwin assets in the release. `astro.config.mjs:120` `softwareVersion` is bumped by hand. | Structured data and visible copy disagree. | Derive both from the release assets in `gen-progress.mjs`, which already calls the releases API. |
| W-23 | Comparison omits QANplatform | No match for "QAN" in `docs/site/authored/pq-comparison.md` or `docs/prior-art/pq-l1-comparison-2026-11.md`. qanplatform.com (2026-11-13) markets a quantum-resistant layer 1 with a public testnet. | A reader who knows the field sees a missing row and discounts the page. | Add the row with a dated search; see section 2 for what was observed. |
| W-24 | Contact and identity | `Footer.astro:26` is a personal webmail address; `:42-65` are personal social profiles. No project channel other than GitHub. Already raised as a decision for Eric in `docs/design/audit-2026-11.md:68`. | A protocol site that reads as a personal page; no role address for security or press. | Role addresses on the project domain and one project channel. Owner decision; no personal detail should be added anywhere to fix this. |
| W-25 | Install by pipe-to-shell from a moving branch | `README.md:54-56` fetch `scripts/join-testnet.*` from `master`. | The command a newcomer pastes can change between two readers, and nothing in the line pins or verifies it. | Fetch from a release tag and print the checksum beside the command; `cargo xtask verify-release` already exists for the binary. |
| W-26 | No release history page | No `CHANGELOG.md`; `CONTRIBUTING.md:34-38`: only the newest release stays published. | No public record of what shipped when, outside `git log`. | A `/releases/` page generated from tags. Overlaps #486 (release-log-anchor): build it on that, not beside it. |
| W-27 | README shows no product | `README.md` embeds brand SVGs only; no screenshot of the explorer, wallet or Hub. | A visitor cannot see that the apps exist. | Three screenshots from the visual baselines (`visual-baseline.yml`), regenerated by CI. |
| W-28 | Nothing is measured | `SEO.md:98` and `AUDIT-2026-11-10.md:20`: no Lighthouse run. The repo holds no traffic or search record. | Every growth idea in this report is a hypothesis with no baseline. | A Lighthouse CI job writing JSON to `reports/site/`; Search Console and Bing verification from the owner checklist (`SEO.md:71-75`). |
| W-29 | Explorer lacks the views the site promises | Routes at `apps/explorer/src/server.rs:102-123`: dashboard, blocks, tx, account, mining, validators. No network graph, validator history or signature view. The status app already draws a peer graph (`apps/status/src/live.html:220`). | The post-quantum property, the project's main difference, is invisible in the explorer. | Lane E. |

### LOW

| ID | Surface | Evidence | Fix |
|---|---|---|---|
| W-30 | README wording | `README.md:283` "a chain that refuses to be worth anything". | "Pre-launch software. The testnet coin has no value." |
| W-31 | "Earn your badge" | `home.mdx:32`; `QuestTrail.astro:5` already says the badge is not a reward. | "Explore, learn, collect a badge": avoid "earn" on a chain site. |
| W-32 | Hub earnings tile | `apps/hub/ui/index.html:57-60` shows coin amounts; no fiat value, price or rate was found in `apps/hub/ui` (grep). | Keep it that way; see section 5. Add a test that fails if a currency symbol appears. |
| W-33 | "Built by Eric" in primary nav | `Header.astro:21`; duplicate of `docs/design/audit-2026-11.md:63`. | Handled by U3. |

## 2. Competitive gap table

Facts observed on 2026-11-13. Each row is what the text extractor returned for
the URL given; "not observed" is not a claim of absence. No competitor figure
is repeated here, because the extractor's numbers could not be checked.

| Site (URL fetched) | First screen: what it says and asks | Live or headline figures | Docs entry and route to a first result | Trust pages observed | Ecosystem and grants |
|---|---|---|---|---|---|
| Ethereum (`ethereum.org/`, `ethereum.org/developers/`) | Ownership message; several entry buttons (start, wallet, apps, build) | Headline figures shown | Developer portal by intent (learn, tools, docs, research); scaffold command and code sandboxes | Bug bounty, security docs, roadmap, translations | Grants page, community hub |
| Solana (`solana.com/`, `/docs`, `/developers`, `/docs/intro/quick-start`) | NOT FETCHED (HTTP 429) | NOT FETCHED | NOT FETCHED | NOT FETCHED | NOT FETCHED |
| Sui (`sui.io/`, `docs.sui.io/`) | AI-transactions message; two buttons (docs, wallet) | One figure shown | Sections: getting started, develop, references, node operators; a "Hello, World" page; MCP server listed | Security, bug bounty, network status, incident history | Programs and funding, RFPs, research awards |
| Aptos (`aptos.dev/`; landing NOT FETCHED, 429) | NOT FETCHED | NOT FETCHED | Sections: getting started, tools, smart contracts, resources; two first-result guides (deploy a module, use the TypeScript SDK); SDKs in TypeScript, Python, Go, Rust; llms.txt and MCP | Not observed on the docs home | "Apply for a grant" link |
| Kaspa (`kaspa.org/`) | Proof-of-work blockDAG message; three buttons (start, wallet, buy) | None observed | Build section and GitHub link; no docs site observed | None observed | None observed |
| Avalanche (`avalanche.com/`, `build.avax.network/docs`) | Business message; two buttons (contact, build) | None on landing; links to a stats page | Docs per chain; faucet and console linked | Status page | Grants page, academy, events |
| NEAR (`near.org/`, `docs.near.org/`) | Agent-economy message; two buttons (swap, build) | Headline figures shown (static) | Docs by task (contracts, apps, protocol, data); `cargo near new` and `npx create-near-app`; llms.txt and docs MCP | Roadmap; status and bounty not observed | Community channels; grants not observed |
| QANplatform (`qanplatform.com/`, `docs.qanplatform.com/`) | Quantum-resistant, multi-language contracts; token and testnet buttons | Static comparison figures, none live | Testnet setup in four numbered steps (wallet, signer, key, faucet); names CRYSTALS-Dilithium in its cross-signer | Named third-party audits; roadmap; status not observed | Not observed |
| bitcoin.org (`bitcoin.org/en/`, `developer.bitcoin.org/`) | Payment-network message; three buttons (start, wallet, buy) | None; links to price, fees, status | Guides, reference, examples, glossary; first example needs a full node | Network status link; scam warning | Donation request; 30+ languages |
| **Maya2C** (`maya2c.dev`, source at `c7ebb4d2`) | Slogan plus one factual line; three buttons (`Hero3D.astro:22-29`) | Wired to live sources, all blank while paused (W-04) | 20 flat guides plus generated reference; five-section source build (`quickstart.mdx:19-148`) | Security policy, hall of fame, health page, roadmap; status host down (W-03) | Ecosystem explorer; no grants page; GitHub Discussions |

### Maya2C scores, with the rubric

Scores are given for Maya2C only. Competitor scores would need rendered pages.

| Rubric | One point each for | Score | Why |
|---|---|---|---|
| R1 First-screen clarity (0-5) | category in plain words; one dominant action; true network state; no empty placeholders; navigation at every width | 1 | "Ledger", "transaction" and "post-quantum signatures" name the category. Three near-equal buttons (`docs/design/audit-2026-11.md:37`). State reads "offline" (W-04). Tiles are dashes. No nav from 800 to 1151 px (`docs/design/audit-2026-11.md:67`). |
| R2 Live data (0-3) | a live figure on the home page; a truthful state when the source is down; a public raw endpoint that answers | 1 | Wired (`Hero3D.astro`), but "offline" for a planned pause, and `/api/status` returned 530. |
| R3 Route to a first result (0-3) | a no-install route; a one-command local route in the docs; install commands that resolve | 0 | `/experience/` is "back with the next genesis" (`home.mdx:72`); the kit is undocumented on the site (W-11); npm and PyPI return 404 (W-05). |
| R4 Docs structure (0-3) | grouped by audience; reference separated from guides; machine-readable index | 2 | Flat guides (W-19); reference is separate; `llms.txt` exists. |
| R5 Structured data (0-5) | Organization; WebSite; BreadcrumbList; article with `dateModified`; FAQPage | 3 | First three present (`astro.config.mjs:99-116`, `Head.astro:27-32`); W-13 for the rest. |
| R6 Trust pages (0-5) | reachable status; security policy; audit status stated; roadmap; incident history | 4 | Status host down (W-03). The audit statement exists but is contradicted (W-01). |
| R7 Ecosystem (0-3) | app directory; funding or grants page; community channel | 2 | No grants page. |

### Where Maya2C differs, and where it does not

Only what `docs/prior-art/` supports:

- Every default-suite transaction carries ML-DSA-65 and SLH-DSA-SHA2-128s
  signatures, and both must verify. The dated search found chains pairing a
  classical scheme with a post-quantum one, and none requiring two
  post-quantum families (`docs/prior-art/pq-l1-comparison-2026-11.md`,
  "Where Maya2C differs", item 1; searched 2026-11-01). That file's own safe
  wording is the sentence above. It forbids "first", "only", "most secure".
- QANplatform's docs name CRYSTALS-Dilithium in a cross-signer
  (`docs.qanplatform.com`, 2026-11-13). That is one lattice scheme; whether a
  classical signature sits beside it was not established here. This needs a
  proper row with a dated search before the site says anything (W-23).
- Others are ahead on time in production, live mainnets, audits and installed
  base (`docs/prior-art/pq-l1-comparison-2026-11.md`, "Where others are
  ahead"). The site's comparison page already says so; keep it.
- The price is 11,165 bytes of signature per transfer (`README.md:77`). The
  throughput cost is MISSING (same prior-art file, citing ADR-051).

## 3. Implementation lanes

Ordered by impact divided by effort. **Impact**: 3 if the lane closes three or
more CRITICAL or HIGH items, 2 for one or two, 1 for none. **Effort**: 1 for
text and config edits, 2 for new pages or scripts without Rust, 3 for Rust or
a new service.

| Order | Lane | Impact | Effort | Ratio |
|---|---|---|---|---|
| 1 | G: GitHub and README | 3 | 1 | 3.0 |
| 2 | A: Website truth and landing | 3 | 2 | 1.5 |
| 3 | C: DevEx | 3 | 2 | 1.5 |
| 4 | D: Trust center | 2 | 2 | 1.0 |
| 5 | F: Ecosystem and grants | 1 | 1 | 1.0 |
| 6 | H: Growth | 1 | 1 | 1.0 |
| 7 | E: Explorer dashboards | 2 | 3 | 0.67 |
| 8 | B: SEO | 1 | 2 | 0.5 |
| 9 | I: Traction dashboard | 1 | 2 | 0.5 |

### Lane G: GitHub and README (closes W-01, W-02, W-06, W-07, W-10, W-25, W-27, W-30)

- **Files:** `D:\Maya2C\README.md`, `D:\Maya2C\CONTRIBUTING.md`,
  `D:\Maya2C\.github\ISSUE_TEMPLATE\`, a new `D:\Maya2C\xtask\src\readme_facts.rs`
  (plus its line in `xtask/src/main.rs`), `D:\Maya2C\docs\brand\`.
- **Owner actions:** repository description and topics; opening the starter
  issues.
- **Acceptance:** README and FAQ carry the same audit-and-mainnet sentence;
  `cargo xtask readme-facts --check` fails when the count block differs from
  `features.toml`; no badge in the README points at a host that can be down by
  design; the "good first issue" link returns at least five open issues; the
  install line names a tag; three screenshots present with alt text.
- **Conflicts:** #485 adds `xtask/src/trust_facts.rs` and edits
  `xtask/src/main.rs`: build `readme-facts` on its output or land after it.

### Lane A: Website truth and landing (closes W-03, W-04, W-08, W-09, W-19, W-20, W-21, W-22, W-31)

- **Files:** `D:\Maya2C\docs\site\home.mdx`,
  `D:\Maya2C\docs\site\src\components\{Hero3D,Header,StackDiagram,QuantumFacts,Maintenance}.astro`,
  `D:\Maya2C\docs\site\src\data\{ecosystem-apps,facts}.ts`,
  `D:\Maya2C\docs\site\scripts\{gen-health,gen-progress}.mjs`,
  `D:\Maya2C\docs\site\astro.config.mjs` (sidebar and `SoftwareApplication` only).
- **Acceptance:** with `TESTNET_PAUSED = true`, the built home page contains
  the word "paused", a date and a link to the stop report, and no tile renders
  a bare dash; no header link targets a host outside the site while paused;
  "ML-KEM-768" appears nowhere without "fallback"; `health.json` has no
  eight-gate field and no test count older than 14 days; every ecosystem card
  status equals its `features.toml` status (a test in `facts.test.ts` style);
  `check-seo.mjs` still passes.
- **Conflicts:** U3 (`feat/ui-u3-website`, not pushed) redesigns the same
  components: deliver these as content requirements to that lane, not as a
  parallel PR. #490, #474, #433 and #486 edit `astro.config.mjs`; #433 edits
  `site.yml` and adds a live-network page that should absorb the paused state.

### Lane C: DevEx (closes W-05, W-11, W-12)

- **Files:** `D:\Maya2C\docs\site\authored\{quickstart.mdx,api.md,wallet.mdx}`,
  `D:\Maya2C\sdks\{sdk-js,python,rust,go}\README.md`,
  `D:\Maya2C\sdks\python\pyproject.toml` (new),
  `D:\Maya2C\.github\workflows\sdks.yml` (install-line check),
  `D:\Maya2C\docs\ecosystem\testnet-kit.md`.
- **Scope, smallest version of each ask:**
  - One-click node, validator, wallet: already exist as `cargo xtask kit up`,
    `cargo xtask node` and the join scripts. The work is documentation.
  - SDK wizard: one page with a language picker reusing `InstallPicker.astro`,
    not a new tool.
  - API explorer: render the OpenAPI file that #414 adds
    (`apps/explorer/openapi`) on a static page. Do not start before #414.
  - Playground: `/experience/` and `/learn/signatures-live/` already run
    signing in the browser through `sdk-wasm`. Extend those to build and
    verify a transaction offline. A hosted contract playground is a new
    service: BACKLOG, not this lane.
- **Acceptance:** the quickstart's first code block is one command and the
  page states a measured time from a recorded run, or says MISSING; every
  install line in `sdks/*/README.md` either resolves in CI or sits under a
  heading "Not published yet"; each SDK README has a complete send-a-transfer
  example checked by `cargo xtask sdk-e2e`.
- **Conflicts:** #498 (C++ SDK), #410 and #446 (edit `docs/site/authored`),
  #414 (API). Publishing packages needs `APPROVED: publish sdk`.

### Lane D: Trust center (closes W-03 with A, W-09, W-26, W-28)

- **Files:** `D:\Maya2C\docs\site\src\pages\trust.astro` (new),
  `D:\Maya2C\docs\site\scripts\gen-progress.mjs`,
  `D:\Maya2C\.github\workflows\site.yml` (Lighthouse step).
- **Content, each line with its source or the word GAP:** status and incident
  history (`public/health/incidents.json`); audit status (the W-01 sentence);
  releases with provenance (`reproducible-build.yml`, code-signing policy);
  CI state and merged changes (`progress.json`, already fetched every six
  hours by `site.yml:25`); contributors (`public/badges/contributors.json`);
  benchmarks (`reports/bench/`, each with commit, hardware and command, and
  the sentence that all runs are one machine on localhost); supply-chain
  state (`cargo deny` blocking, `cargo vet` report-only).
- **Acceptance:** every figure on the page has a link to its source file or
  workflow run; a figure whose source is missing renders "GAP"; no figure is
  typed into the page; the page builds with the network offline.
- **Conflicts:** #485 produces exactly this evidence-or-GAP sheet as an xtask:
  the page should render its output. #444 (status v2), #480 (invariant monitor,
  validator ledger), #486 (release log, signed incidents, canary), #454
  (supply audit) each supply a row. This lane is a page over those, and should
  land after #485.

### Lane F: Ecosystem, partners and grants

- **Files:** `D:\Maya2C\docs\site\src\pages\ecosystem\index.astro` (new),
  `D:\Maya2C\docs\site\src\data\ecosystem-apps.ts`,
  `D:\Maya2C\docs\ecosystem\CATALOG.md`.
- **Acceptance:** the page lists apps from `CATALOG.md` with their ledger
  status; it has a "Partners" section that is empty and says so until a
  partner exists in writing; it has a "Funding" section that states there is
  no grants programme and no token sale, and lists what a builder does get
  (credit, badge, review); no logo of a third party appears without that
  party's written consent on file.
- **Conflicts:** #474 (`/design/` explorer), #490 (component kit).

### Lane H: Growth, safe version (see section 5)

- **Files:** `D:\Maya2C\docs\growth\ambassador-design.md` (new, design only),
  `D:\Maya2C\docs\site\authored\contributors.md`.
- **Acceptance:** the design pays nothing, in any asset; recognition is the
  existing badge system; no page contains a price, a return or an earnings
  estimate; a reviewer checklist from section 5 is part of the doc.
- **Conflicts:** none.

### Lane E: Explorer dashboards (closes W-14, W-15, W-29)

- **Files:** `D:\Maya2C\apps\explorer\src\{ui,server,chart,model}.rs`,
  `D:\Maya2C\apps\explorer\assets\`, `D:\Maya2C\apps\explorer\tests\`.
  The explorer is an app, not linked by the node, so this is not post-genesis.
- **Views, each from data the indexer already stores or the node already
  serves:** network graph (reuse the status app's peer view,
  `apps/status/src/live.html:220`); transaction visualiser (the two
  signatures, their sizes and verify result); validator health (committee,
  missed rounds, history); post-quantum dashboard (suite per transaction,
  signature bytes per block, share of default-suite transactions).
- **Acceptance:** each view has a server test in `apps/explorer/tests/`;
  each figure names its RPC or table source in a tooltip; the chain name comes
  from the node; every page has `lang`, description, canonical and an absolute
  preview image; pages render without JavaScript.
- **Conflicts:** heavy. #488 (tokens), #484 (explain block), #414 (API v1),
  #410 (Bech32m) all edit `apps/explorer/src`. Start only after those merge;
  U6 in `docs/design/forged-light-2.md` section 5.4 owns the look.

### Lane B: SEO (closes W-13, W-23; W-14 with E)

- **Files:** `D:\Maya2C\docs\site\src\components\Head.astro`,
  `D:\Maya2C\docs\site\scripts\{check-seo,ingest}.mjs`,
  `D:\Maya2C\docs\site\authored\pq-comparison.md`,
  `D:\Maya2C\docs\prior-art\`, `D:\Maya2C\docs\site\SEO.md`.
- **Schema:** `FAQPage` on the FAQ; `dateModified` on `TechArticle`;
  `HowTo` on quickstart and help-run; `sameAs` extended only with real
  project profiles.
- **Keyword clusters** (extend `SEO.md:52-62`; a term is used only where the
  page supports it): post-quantum blockchain; ML-DSA and SLH-DSA signatures;
  hybrid post-quantum signatures; ML-KEM transport; DAG-BFT finality; run a
  post-quantum node; post-quantum wallet.
- **Comparison pages:** one page per chain already in the dated table, built
  from the prior-art file, never hand-written. Each states the search date,
  where the other chain is ahead, and carries no ranking word. No page is
  published for a chain without a dated row.
- **Programmatic pages:** one per `features.toml` subsystem (name, class,
  status, the named test, the ADR), and one per ADR. These are generated from
  files that CI already checks, so they cannot overstate.
- **Acceptance:** `check-seo.mjs` fails without `FAQPage` on the FAQ and
  without `dateModified` on a guide; every comparison page links its prior-art
  source and `cargo xtask claims-check` passes; QANplatform has a dated row.
- **Conflicts:** #490 and #474 (`docs/site/src`); #477 adds prior-art files.
- **Hypothesis, not a forecast:** question-form queries about post-quantum
  signatures will start to show the FAQ. Measure: Search Console impressions
  for `/guides/faq/`, four weeks before and after, once the owner verifies the
  property (`SEO.md:71-72`). No baseline exists today (W-28).

### Lane I: Traction dashboard

- **Files:** `D:\Maya2C\docs\site\scripts\gen-progress.mjs`,
  `D:\Maya2C\docs\site\src\pages\traction.astro` (new).
- **Sources only:** GitHub API (commits, merged PRs, releases, contributors,
  stars, forks), `features.toml` history from git, CI run history, and, once a
  network runs, the explorer's `/api/stats` (blocks, transactions, validators,
  peers). `cargo xtask eco-metrics` already exists; read it before adding a
  script.
- **Acceptance:** every series is a count with its source and fetch time; no
  projection, no target line, no price, no market value; when a source is
  unreachable the series ends and says so; the page states the network is
  paused whenever it is.
- **Conflicts:** #433 (live network page), #444 (SLO view). Fold into Lane D
  if both would otherwise show the same counts.

## 4. Verifiable claims

Each claim the site may make, with the evidence that must sit beside it. A
claim whose evidence column says MISSING must not be published.

| Claim (safe wording) | Evidence source |
|---|---|
| Every default-suite transaction carries ML-DSA-65 and SLH-DSA-SHA2-128s signatures, and both must verify. | `features.toml` `hybrid-signature-transaction-validation` (verified); invariant 31; `docs/hybrid-signatures.md` |
| A transfer carries 11,165 bytes of signature (3,309 + 7,856). | `docs/hybrid-signatures.md`; `QuantumFacts.astro:28` |
| Peer connections add ML-KEM-1024, with ML-KEM-768 as fallback, on top of Noise. Not externally reviewed. | `docs/pq-transport.md`; `features.toml` `p2p-mlkem-1024` (working) |
| Blocks are final under DAG-BFT while fewer than a third of validators are faulty. | `features.toml` `dag-bft` (verified); ADR-027; `crates/node/tests/bft_node_tests.rs` |
| The testnet ran 7 days 15 hours, blocks 0 to 393,812, with no halt, and was stopped on purpose on 2026-10-28. | `reports/sessions/2026-10-28-testnet-stopped.md` |
| Validators in that run were operated by the project. | `Hero3D.astro:36` ("project-run"); ADR-042 |
| No external security audit has been completed. | `docs/site/authored/faq.md:83`; `docs/security-audit.md` |
| Mainnet has not launched. | `docs/mainnet-v1-plan.md`; `public/progress.json` gates |
| N of 10 mainnet gates are met. | `public/progress.json` `gates`, generated from `docs/mainnet-v1-plan.md` |
| N subsystems are verified; each names a test that exists. | `features.toml`; CI job "reality ledger" (`cargo xtask coverage --verify-targets`) |
| Money arithmetic is checked by Kani proofs. | `kani-heavy.yml`; `cargo xtask proof-ratchet --check` |
| Releases carry build provenance. | `reproducible-build.yml`; `docs/site/authored/code-signing-policy.md` |
| The served site matches the built site. | `site-integrity.yml` (hourly comparison with the attested manifest) |
| Apache-2.0 code; the name and emblem are not licensed to forks. | `LICENSE`; `BRAND.md` |
| No token is for sale. | `home.mdx:95` |
| Mining is designed as a reward lane that never orders blocks; it has not run on a public network. | ADR-044; ADR-068; prior-art comparison, item 5 |
| Finalized TPS figures are single-machine, localhost runs with the stated mix. | `reports/bench/` (each with commit, hardware, command) |
| Throughput cost of two signatures | MISSING (ADR-051) |
| Multi-machine or wide-area performance | MISSING: no such run exists |
| Lighthouse or Core Web Vitals scores | MISSING (`SEO.md:98`) |
| Comparison with QANplatform | MISSING: no dated row (W-23) |
| Any "first", "only" or ranking statement | Not permitted: `docs/prior-art/pq-l1-comparison-2026-11.md` forbids it |

## 5. Legal risks in the growth asks

Not legal advice. These are the points where a lawyer's review is needed
before anything ships, and the version that avoids the question.

| Ask | Risk | Safe version |
|---|---|---|
| Referral payouts | Paying people in a token, or in anything of value, for bringing in others who buy or hold can make the token look like an investment sold through promoters, and may trigger promoter-disclosure rules. A multi-level structure adds pyramid-scheme exposure. | No payout in any asset. Recognition only: the existing contributor badges (`/guides/contributors/`), named credit, early access to test builds. One level, no rewards for recruits' activity. |
| Ambassador programme | An ambassador who is rewarded and posts without saying so is undisclosed paid promotion; statements they make about value are attributed to the project. | Volunteers, unpaid, with a written code: disclose the role in every post, never mention price or returns, use only the claims in section 4. |
| Investor dashboard showing token value | Publishing a price, a market value or a projected value is the centre of an investment-contract argument, and the project has stated that no token is for sale (`home.mdx:95`). | Do not build it. The traction page (Lane I) shows counts of work and network activity only. |
| Income or earnings claims for mining or staking | Earnings estimates are forward-looking promises. `start-mining.mdx:167` already says no profit is promised or estimated. | Keep that sentence. The Hub shows coins received, never a fiat value, rate of return or payback time (W-32). |
| "Traction" numbers aimed at investors | A selective or projected figure shown to prospective funders is a representation they may rely on. | Counts from public sources with fetch time; no projections; the same page for everyone. |
| Partner logos | Showing a logo implies endorsement. | Written consent on file per logo, or no logo. |
| Comparison pages | A false or unsupported comparative statement about a named competitor. | Dated, sourced rows only; the page states where the other chain is ahead (Lane B). |

## Open pull requests that touch these surfaces

Listed so no lane duplicates them (`gh pr list`, 2026-11-13).

| PR | What it changes | Lanes affected |
|---|---|---|
| #478 (merged) | U0 audit, Forged Light 2 direction, mockups | Visual findings live there |
| #488 | U1 design tokens across explorer, Hub, wallet, chat | E, W-17 |
| #490 | U2 component kit, `/design-system/` page | A, B, F |
| U3 `feat/ui-u3-website` (not pushed) | Website redesign | A |
| #474 | `/design/` explorer for proposed ADRs | A, B, F |
| #433 | Live network page; edits `site.yml` | A, D, I |
| #444 | Status v2: incident history, gates, SLO view | D, I, W-15 |
| #484 | Explorer: explain each block and transaction | E |
| #414 | Public indexer API v1 with OpenAPI | C, E |
| #410 | Bech32m on pool login, explorer, faucet | C, E, W-18 |
| #446 | Wallet clear signing, address book | C |
| #485 | `xtask trust-facts`: evidence or GAP per line | D, G |
| #486 | Release log anchor, signed incidents, canary | D, W-26 |
| #480 | Invariant monitor, validator ledger | D |
| #454 | Supply recomputed from public RPC | D |
| #495 | Chat mini-apps | W-18 |
| #498 | C++ SDK | C |
| #492 | ADR-137 hardware intelligence (Hub tuner) | Hub copy, W-32 |
| #491 | Hub GPU auto-tuner | Hub copy, W-32 |
| #477 | ADR-135 umbrella, adds prior-art files | B |
