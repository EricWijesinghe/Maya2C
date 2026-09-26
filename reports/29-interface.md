# Report 29 — Interface: design system, wallet flows, explorer

Master Prompt 29. "World-class" was to be defined by measurements. This
report says which measurements exist and which do not. **Of the four DONE WHEN
conditions, one is met, one is met with a stated limit, and two are not met.**

| DONE WHEN | Status |
|---|---|
| Design system and visual regression tests run in CI | **Met.** Token, contrast, flow and catalog tests run in the `test` job; screenshot diffs run in the new `visual` job |
| All wallet flows pass e2e tests | **Not met.** The flows are specified as state graphs and checked mechanically; the rendered wallet flows have no e2e run (the WebDriver suite has never executed — `apps/wallet-gui/e2e/README.md`) |
| Performance budgets pass on the reference devices | **Partly.** Byte and request budgets are enforced in CI; no Lighthouse run, no device |
| Usability results recorded | **Not met.** No study was run. The protocol is below; results cannot be invented |

## 1. Design system — `crates/design-system`, `design/`

- **Tokens.** Eleven color roles in two themes (`calm`, light and the default;
  `command`, the dark "tactical command center" look), a 1.25 type scale, a
  4 px spacing grid, three motion durations that drop to 0 under
  `prefers-reduced-motion`, and a 44 px touch target. `design/tokens.css` is
  generated and a test fails if the committed file drifts.
- **WCAG 2.2 AA, measured.** Every rendered text/background pair is listed in
  `REQUIRED_PAIRS` and checked by `every_required_pair_meets_wcag_aa_in_both_themes`.
  The formula is pinned to reference points (21:1 black/white; `#767676` on
  white passes at 4.54, `#777777` fails). Measured:

  | Pair | calm | command | AA min |
  |---|---|---|---|
  | fg / bg | 17.76 | 16.93 | 4.5 |
  | muted / panel (worst text pair) | 6.17 | 7.14 | 4.5 |
  | on-accent / accent (button) | 6.25 | 11.71 | 4.5 |
  | danger / panel | 5.99 | 6.56 | 4.5 |
  | success / panel | 5.42 | 11.06 | 4.5 |
  | focus / panel (non-text) | 6.14 | 14.22 | 3.0 |

  Contrast is one criterion of WCAG 2.2 AA. Keyboard order, screen-reader
  labels, target size in the live apps and error identification were not
  audited. "Both themes meet WCAG 2.2 AA" is claimed **for color contrast only**.
- **The explorer's old palette already passed.** Measured before replacing it:
  muted/panel 5.32, error/bg 8.11, accent/panel 8.87, button 10.24. The
  migration was for consistency, not a contrast fix.
- **Component docs.** `design/specimen.html`, generated from the crate, shows
  every component in both themes plus a right-to-left panel. It is also the
  visual-regression fixture.
- **Visual regression.** `design/visual/regress.mjs` screenshots each specimen
  section in Chromium and compares pixels against `design/visual/baseline/`.
  The tolerance is 32 per channel, and a section fails when more than 0.1% of
  its pixels change. The run and the baselines are pinned to
  `mcr.microsoft.com/playwright:v1.56.1-noble`, because font rasterisation
  differs between hosts.
  - Measured in that image: a re-run gives 0.000% change in all 3 sections.
  - Lightening only the calm `--danger` token (`#b3261e` → `#d05a52`) fails
    calm-en at 0.479% and calm-ar at 0.218%, and leaves command-en at 0.000%.
    The diff therefore catches a one-token change and points at the right
    theme.
- **Adoption.** The explorer renders with the shared tokens (`data-theme="command"`),
  gains a `:focus-visible` ring and a touch-sized button. The wallet UI
  (`apps/wallet-gui/ui/styles.css`) and dashboard still carry their own
  stylesheets; the portal and dev hub are README-only directories with no UI
  to adopt anything. **One of four interfaces uses the design system.**

## 2. Wallet UX — `design-system::flows`, `design-system::guard`

- **Ten flows as state graphs:**
  - onboarding without a seed phrase (passkey plus guardians);
  - receive, send and swap;
  - cross-chain intent, with an expiry and refund path;
  - recovery (guardians plus a timelock);
  - vault settings (a timelocked change);
  - app connection and clear-signing review;
  - the approval manager.

  `no_wallet_flow_has_a_dead_end` checks four rules on each flow:

  1. every state is reachable;
  2. `done` is reachable from every state, including `error`;
  3. `cancelled` is reachable from every state, except a screen that only
     reports an outcome already committed;
  4. every flow has an `error` state.

  A second test builds one broken flow per rule and checks that each is caught.
  This proves the **specification** has no dead ends. It says nothing about
  the screens until they are tested against it.
- **Security UX detectors, each with its rules pinned by tests:**
  - **Address poisoning.** An address that is not a contact, but matches a
    contact's first and last 4 hex characters, is flagged. That is the pattern
    truncated display invites.
  - **Look-alike tokens.** A token is identified by its contract, never by its
    symbol. An exact `USDC` on the wrong contract is flagged, and so are
    Cyrillic `USDС`, a zero-width `U​SDC` and lower-case `usdc`. `USDT` is not
    flagged.
  - **Phishing domains.** Allow-list entries and their subdomains are trusted.
    `xn--` labels are flagged. The following are look-alikes: an allowed name
    used as a prefix (`maya2c.io.evil.com`), a homoglyph skeleton match
    (Cyrillic `mауа2с.io`), and edit distance ≤ 2 (`rnaya2c.io`, `maya2c.co`).
    `evilmaya2c.io` is deliberately **not** flagged: it is at distance 4 and is
    not a subdomain. Catching it would need a registrable-domain list.
  - None of the detectors blocks by itself. A false block is a dead end, so
    each one warns, and the flows route the warning to "edit" or "continue".
- **Approval manager.** It exists as a flow (list, select, sign a revocation).
  Nothing indexes approvals yet, so there is nothing for it to list.

## 3. Explorer

- **Plain language.** The transaction page leads with one sentence, for example
  `5ad1a05a…a0beef sent 1,500,000 base units to 2 recipients in block 1204.`
  The field table is behind a "Technical view" disclosure.
  - Amounts are in **base units**, because `chain/maya2c-testnet.json` has no
    ratified symbol or decimals, and writing "10 MAYA" would invent both.
  - Recipients are counted, not named, because the index stores output totals
    only.
  - The brief's example ("Alice swapped 10 MAYA for 25 USDC on Pool X") needs
    decoded contract calls, token metadata and a name service, and none of
    those is indexed.
- **Contract safety report (MP23) and quantum-exposure status (MP28) on
  explorer pages.** Neither is shown. The explorer indexes no contracts. For
  accounts, the exposure question does not apply the way it does on Bitcoin:
  a Maya2C account signs with a post-quantum or hybrid suite. What an account
  page *should* show is its registered suite, and the index does not store it.
- **3D DAG view.** Not present in `apps/explorer`, so there is no 2D fallback to
  keep. Recorded rather than built.

## 4. Performance and reach

- **Budgets enforced in CI** (`apps/explorer/tests/budget_tests.rs`), measured:

  | Page | Size | Budget |
  |---|---|---|
  | Account page with 50 transactions | 18,356 B | 65,536 B |
  | Transaction page | 7,945 B | 24,576 B |

  The render-blocking request count beyond the document must be 0. CSS and
  tokens are inline, and the only script is inline.
- **Not measured.** The brief asks for first load and interaction time on a
  mid-range Android phone over slow 4G, measured with Lighthouse and real
  devices. Lighthouse is not installed here and no device is attached. The
  byte budgets are a proxy: 64 KiB is about 0.33 s of transfer at Lighthouse's
  1.6 Mbit/s slow-4G profile, before compression and the 150 ms RTT.
- **Localization.** There are 10 locales: en, es, pt-BR, zh-CN, hi, ru, ja,
  ko, tr and ar. Arabic is right-to-left, and the specimen's RTL panel is in
  the screenshot baselines.
  - The array type makes a missing string a compile error.
  - A test checks that every locale keeps its `{known}` placeholders.
  - **The translations are unreviewed drafts** (`i18n::REVIEWED = false`).
    A native-speaker review is a launch gate.
  - The locale choice was not derived from developer-location data; it
    should be.
  - Only 12 keys exist: the security warnings and core actions. The apps'
    other strings are English literals.
  - The RTL baseline shows a real defect: English sentences inside an RTL
    container put their final period on the wrong side. Every untranslated
    string will do this.
- **Offline.** Not built. The catalog has the "Offline: showing your last known
  balance" string; no service worker or cache exists in any interface.

## 5. User testing — not run

No participants were recruited, and nobody was observed. The brief asks for at
least 20 moderated sessions, and a report written without them would be
fiction. The protocol to run:

- **Tasks (the top five):**
  1. onboard without a seed phrase;
  2. receive;
  3. send to a contact, with a poisoned look-alike planted in the history;
  4. swap, with a look-alike token in the list;
  5. recover on a new device.
- **Participants.** 20 or more, split across three experience levels (never
  held crypto, holds but has not used DeFi, uses DeFi weekly) and at least
  four countries that use at least two of the catalog's scripts, including
  Arabic.
- **Per task, record:**
  - success (unaided, aided or failed);
  - time on task;
  - error count, and **whether the planted poisoning or look-alike was
    caught**;
  - SEQ (1–7) after each task, and SUS at the end.
- **Beat bar.** The only UI bar in `docs/strategy/BEAT_BARS.md` is blind
  signing, and it is "not researched". MP21 set no task-success or SUS bar, so
  there is nothing yet to beat. The first study sets the baseline.

## What would make this "world-class" by the brief's definition

1. Run the usability study above and record the numbers here.
2. Move the wallet UI and dashboard onto `design/tokens.css`, and build the
   portal and dev hub on it from the start.
3. A Lighthouse CI job against the explorer, served with fixture data, plus
   one physical mid-range Android device.
4. Test the rendered wallet screens against the flow graphs (the WebDriver
   suite, once a driver exists).
5. A native review of the ten catalogs, and extraction of every UI literal.
