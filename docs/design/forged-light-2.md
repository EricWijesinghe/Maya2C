# Forged Light 2: one design system for every Maya2C surface

Status: **direction, awaiting Eric's approval** (plan: `ui-greatest-upgrade`,
job U0). Nothing here is implemented. The evidence is in
[audit-2026-11.md](audit-2026-11.md); the mockups are in
[mockups/](mockups/) with screenshots in [shots/](shots/).

Forged Light (2026-10-05) chose the right materials: the emblem's teal,
bronze and steel on an abyss ground, Manrope for display, Inter for reading,
JetBrains Mono for data. Five surfaces then each wrote their own stylesheet
from those materials, and the result reads as five products. Forged Light 2
keeps the materials and replaces the five stylesheets with one token file and
one component kit, then rebuilds each surface's key screen on them.

## 1. Principles

1. **One voice.** A visitor who opens the website, the explorer, the Hub, the
   wallet and the chat sees one product. Same tokens, same components, same
   words for the same states.
2. **Proof before persuasion.** Real numbers are the brand's strongest asset
   (Standing Order 1, invariant: `—` for a missing value). The first screen of
   every surface shows the live figures that matter there, each with its
   source. No figure is ever decorative.
3. **One message, one action per screen.** The website hero has one headline
   and one primary button. The wallet confirm screen has one sentence and one
   button. A screen with two primaries has no primary.
4. **Quiet surfaces, one bright thing.** Cards are plain surfaces; glow,
   gradient and motion are spent on the single element that carries the
   screen (the 3D core, the gauge, the clear-sign sentence). Everything else
   rests.
5. **Meaning decides colour.** Teal means live/interactive, bronze means
   pending/attention, green means verified, red means danger, violet means
   research. Position (`nth-child`), category and decoration never pick a
   colour.
6. **Graphics over text, then text that fits.** Maps, timelines, orbs and
   gauges stay; each bubble is clickable; one short line per item. Where an
   item has more than one line of copy, it is a card, not a bubble.
7. **Works at 390 px first.** Every layout is designed at phone width and
   then widened. A sidebar is a drawer under 760 px. Nothing is clipped.
8. **Motion is a state change.** Something moves because something happened:
   a block arrived, a share was accepted, a transfer was signed. Nothing loops
   forever except the one live indicator per screen. `prefers-reduced-motion`
   turns all of it off.
9. **Nothing claims what a test does not pin.** No "first", "only",
   "unprecedented" (`cargo xtask claims-check`); the site follows every
   upgrade in the same PR.

## 2. What stays, what changes

| Stays | Changes |
|---|---|
| The emblem, everywhere, as the mark | The wordmark is plain `--text` next to it on every surface: no gradient text, no letter-spacing, no "Hub" in gold |
| Teal / bronze / steel / abyss, from the emblem | Eleven named roles (§3.1) replace ~580 colour literals; Tailwind reds, greens, violets and the explorer's cyan/indigo and calm/command green are deleted |
| Manrope + Inter + JetBrains Mono, self-hosted | The explorer loads them (today it names Inter and never loads it); mono is reserved for hashes, addresses, code and tabular figures |
| "Maya2C" one word | App names become `Maya2C Hub`, `Maya2C Wallet`, `Maya2C Chat` (decision for Eric) |
| Real numbers, `—` when missing | A missing number also gets a one-line reason (`paused since 11-10`, `explorer unreachable`), in muted text, never in the hero slot |
| Graphics over text: 3D hero, orbs, journey, map, gauge | Orb rows only for 4 or 6 items; longer lists are tile grids; one chart palette; the stack diagram drops the hue wheel |
| Dark as the brand's home theme | Light is a first-class theme on all five surfaces, including the Tauri apps and the explorer (its toggle works or goes) |
| Soft geometry, glass with depth | Four radii instead of fifteen; glass only on floating chrome (header, popovers); cards are solid surfaces so text never sits on blur |
| Spring easing on interaction | Infinite sweeps on rules and dividers, the explorer scan line and arcs, and scroll-driven reveals are removed |
| Reduced-motion honoured | One technique everywhere: durations to 1 ms (the wallet's), not `animation: none` (which breaks `animation-fill-mode: both` end states) |
| Starlight for docs | Reading pages get a plain background; motifs only on home, status and experience |

## 3. Tokens

The source will be `design/tokens.json` (U1), generated into `tokens.css` and
copied into each UI by `cargo xtask design-tokens` with a CI drift check. The
existing `crates/design-system` and `design/tokens.css` (calm/command, green)
are replaced, not extended: nothing consumes them except the explorer's base
layer and `design/specimen.html`, both of which U1/U6 rewrite.

Names are role names. No surface may add a colour outside this list; a new
role is a tokens.json change with a measured ratio. `cargo xtask
design-tokens --literals <dir>` lists every colour a folder still writes by
hand.

§3.1 and §3.7 are current (token revision 2, 2026-11-13). §3.2 to §3.6 are
the first proposal; where they differ from `design/tokens.json` (Inter only,
radii 4/6/8/12, motion 120/180/240 ms on one easing), the token file and
ADR-136's revisions win.

### 3.1 Colour

Dark is the brand's home theme and the default when the OS states no
preference (Eric, 2026-11-12); the OS preference applies otherwise, and an
explicit `data-theme` wins over both. The values below are
`design/tokens.json`, revision 2 (2026-11-13, ADR-136). `cargo xtask
design-tokens --check` compares this table with the token file and fails on
any difference, so the table cannot drift from what ships.

**One meaning per colour.** A reader learns a colour once and then reads it
everywhere without a legend:

| Hue | Means | Never means |
|---|---|---|
| Teal (`accent`) | Trust, secure, the primary action, live | Money or value |
| Bronze (`bronze`) | Value: coins, amounts, proofs, the forge warmth | An error or a warning |
| Green (`success`) | Verified, confirmed, delivered | "Go" for an unverified step |
| Amber (`warn`) | Needs a look: testnet, paused, pending | Failure |
| Coral (`danger`) | Error, failed, irreversible | Decoration |
| Blue (`info`) | Information, research status | A link (links are `accent-text`) |
| Steel (`steel`) | Reference: hashes, heights, secondary figures | State |

**Why the ground was lifted and tinted.** Revision 1 used a hue-free near-black
(`#08090b`) with grey text and a muted teal. Eric's reading was "too dark,
dull". A ground with no hue reads as a switched-off screen, and on a
near-black every accent has to be very bright to register, which is where the
dullness came from: little contrast between the ground and the cards, and
colour that looked dimmed. Revision 2 puts the ground at a deep teal-navy
(`#081c28`), so the brand hue is present before any accent is drawn; steps
the three surfaces up in the same hue so a panel separates from the ground
without a hairline (the gate requires `surface-3` against `bg` at 1.25:1 or
more in dark); and gives the accents more chroma at the same or better
contrast. Light is warm paper (`#faf8f4`) with warm hairlines and shadows,
not a grey inversion of dark, so the light theme carries the same warmth as
the bronze instead of a cool office grey.

**Colour-blind rule.** Colour is never the only carrier of a state. Every
status chip shows an icon and a word ("Verified", "Testnet", "Failed"), and a
chart series has a label at its end or in a legend. The chart series below
are additionally checked: the generator simulates deuteranopia and
protanopia (Machado, Oliveira and Fernandes 2009, severity 1.0) and fails
when the two closest series fall below `chart-distinctness.min-distance`
(OKLab) in either theme. Status colours are never series colours.

| Role | Dark | Light | Use |
|---|---|---|---|
| `--bg` | `#081c28` | `#faf8f4` | Page ground. The only place motifs and the 3D field paint. |
| `--surface-1` | `#0d2734` | `#ffffff` | Cards, panels, table body, inputs. |
| `--surface-2` | `#133342` | `#f4f1eb` | Raised control, hover row, inline code, chips. |
| `--surface-3` | `#1a4051` | `#ece8e0` | Pressed or selected. |
| `--border` | `rgb(150 210 225 / 0.14)` | `rgb(60 45 20 / 0.14)` | Hairlines and card edges. Not a control edge. |
| `--border-strong` | `#6f9aa9` | `rgb(60 45 20 / 0.26)` | Dividers that must be seen, table header rule. |
| `--border-control` | `#5f8998` | `#8d877c` | Input, checkbox and select edges: 3:1 on `bg`, `surface-1`, `surface-2` (SC 1.4.11). |
| `--text` | `#f2f9fa` | `#14201f` | Headlines, values, body. |
| `--text-2` | `#bcd0d7` | `#3d4a49` | Running text, ledes, table cells. |
| `--text-3` | `#91abb5` | `#5f6b69` | Labels, captions, timestamps, sources. Body-size text: 4.5:1 on every surface. |
| `--accent` | `#1fd3cc` | `#0a7568` | Focus ring, live dot, selected state, icons. Non-text. |
| `--accent-hover` | `#52e5dd` | `#075c52` | Hover of an accent control. |
| `--accent-subtle` | `rgb(31 211 204 / 0.14)` | `rgb(10 117 104 / 0.1)` | Selected row, accent chip fill. |
| `--accent-text` | `#52e5dd` | `#075c52` | Links and accent labels at any size. |
| `--accent-grad-from` | `#52e5dd` | `#0a7568` | Primary button, top stop (`--grad-cta`). |
| `--accent-grad-to` | `#0bb5b3` | `#086356` | Primary button, bottom stop. |
| `--accent-highlight` | `rgb(255 255 255 / 0.22)` | `rgb(255 255 255 / 0.18)` | The 1 px inner top edge of the primary button. |
| `--on-accent` | `#04222a` | `#ffffff` | Text on `--accent`, `--accent-hover` and the CTA stops. |
| `--bronze` | `#ffbb6b` | `#8a5a1f` | Value: amounts, coins, proofs. Text-safe on every surface. |
| `--bronze-text` | `#ffd39b` | `#6f4612` | Bronze for long runs of text. |
| `--steel` | `#96b8da` | `#4a5a70` | Reference figures: hashes, heights, IDs. |
| `--success` | `#4ade80` | `#17703a` | Verified, confirmed, delivered. |
| `--warn` | `#ffd43b` | `#855600` | Needs a look: testnet, paused, pending. Never an error. |
| `--danger` | `#ff7f8f` | `#b3261e` | Error, failed, irreversible. |
| `--info` | `#6cb6ff` | `#2f55b8` | Information, research status. |
| `--forge-grad-from` | `#ffd39b` | `#ffd39b` | Forge button (value actions: send, stake), top stop (`--grad-forge`). |
| `--forge-grad-to` | `#f5a04a` | `#f5a04a` | Forge button, bottom stop. |
| `--on-forge` | `#2b1503` | `#2b1503` | Text on the forge stops. |
| `--deco-teal` | `rgb(31 211 204 / 0.16)` | `rgb(10 117 104 / 0.08)` | Decorative only: large glows in `--grad-hero`. Never text, never meaning. |
| `--deco-bronze` | `rgb(255 187 107 / 0.10)` | `rgb(181 103 34 / 0.08)` | Decorative only. |
| `--deco-blue` | `rgb(108 182 255 / 0.10)` | `rgb(47 85 184 / 0.06)` | Decorative only. |
| `--glass` | `rgb(13 39 52 / 0.8)` | `rgb(250 248 244 / 0.82)` | Header and command palette only, with `--glass-blur`. |
| `--shadow-sm` | `0 1px 2px rgb(2 10 16 / 0.45)` | `0 1px 2px rgb(60 45 20 / 0.08)` | Contact shadow. |
| `--shadow-md` | `0 1px 2px rgb(2 10 16 / 0.45), 0 8px 24px -8px rgb(2 10 16 / 0.55)` | `0 1px 2px rgb(60 45 20 / 0.08), 0 8px 24px -8px rgb(60 45 20 / 0.14)` | Raised: hover and selected. |
| `--shadow-lg` | `0 2px 4px rgb(2 10 16 / 0.45), 0 24px 48px -12px rgb(2 10 16 / 0.65)` | `0 2px 4px rgb(60 45 20 / 0.1), 0 24px 48px -12px rgb(60 45 20 / 0.18)` | Floating: header, palette. |
| `--shadow-inset` | `inset 0 1px 0 rgb(255 255 255 / 0.06)` | `inset 0 0 0 0 transparent` | Top-edge light on dark cards; none on paper (written as a zero shadow so it still composes in a `box-shadow` list). |
| `--chart-1` | `#1fd3cc` | `#089192` | Series 1, teal. |
| `--chart-2` | `#ffbb6b` | `#b56722` | Series 2, bronze. |
| `--chart-3` | `#2f6bff` | `#123f99` | Series 3, blue. |
| `--chart-4` | `#f2f9fa` | `#06202a` | Series 4, neutral. |
| `--chart-5` | `#c58cff` | `#7c3aed` | Series 5 (a chart hue, not a role). |
| `--chart-6` | `#d97f4f` | `#733f1c` | Series 6, copper. |

Composed tokens, declared in each theme block because a `var()` resolves where
it is declared (a nested `data-theme` must re-declare them): `--grad-cta`
(the two accent stops, top to bottom), `--grad-forge` (the two forge stops),
`--grad-hero` (a teal glow from the top and a bronze one from the lower
right over `--surface-1` to `--bg`), `--chart-grid` (`--border`) and
`--chart-axis` (`--text-3`).

Every measured pair is listed in `contrast-pairs` and checked in both themes
on every generator run, which prints each ratio. The generator also refuses a
file that leaves out a pair of its required matrix: every text role
(`text`, `text-2`, `text-3`, `accent-text`, `bronze`, `bronze-text`,
`steel`, `success`, `warn`, `danger`, `info`) as body text (4.5:1) on all
four grounds; `accent` as non-text (3:1) on all four; `on-accent` on the
accent, its hover and both CTA stops; `on-forge` on both forge stops;
`border-control` and every chart series as non-text on `bg` and
`surface-1`; and the dark separation pair. Ratios are not copied here: the
gate's output is the record. The chart series in the 2026-11-13 plan were
given in two different orders per theme; they are stored in one hue order
(teal, bronze, blue, neutral, violet, copper) so a series keeps its hue
family when the theme changes, and dark gained `#d97f4f` so both themes have
six. Two plan values were lifted in lightness only, same hue, because the
gate measured them below 4.5:1 on `surface-3`: dark `text-3` `#8fa9b3` to
`#91abb5`, and dark `danger` `#ff7183` to `#ff7f8f`. No violet or quantum
role exists; that is Eric's decision (§8).

### 3.2 Type

Self-hosted variable fonts on every surface (`@fontsource-variable` on the
site; the Hub's `fonts/` layout on the apps and the explorer).

| Token | Value |
|---|---|
| `--font-display` | `"Manrope Variable", "Manrope", system-ui, sans-serif` |
| `--font-body` | `"Inter Variable", "Inter", system-ui, -apple-system, "Segoe UI", sans-serif` |
| `--font-mono` | `"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, Consolas, monospace` |

Scale (px; line-height; weight; face). One scale for all surfaces; the apps
use the same steps with the body at 15.

| Step | Size / line | Weight | Face | Used for |
|---|---|---|---|---|
| `display-1` | 56 / 1.04, phones 40 | 720 | display | Website hero h1 only |
| `display-2` | 40 / 1.1, phones 30 | 650 | display | Section titles, wallet hero amount, Hub gauge value (tabular) |
| `title-1` | 28 / 1.2 | 650 | display | Page titles in apps, h2 in docs |
| `title-2` | 20 / 1.3 | 620 | display | Card titles, h3, clear-sign sentence (24 on the confirm screen) |
| `body-lg` | 17 / 1.7 | 400 | body | Documentation body (unchanged) |
| `body` | 15 / 1.55 | 400 | body | App body, site UI copy |
| `body-sm` | 13 / 1.5 | 400 | body | Hints, sources, table cells |
| `label` | 12 / 1.2 | 600 | body | Caps labels: `letter-spacing: .08em; text-transform: uppercase` |
| `mono` | 13 / 1.5 | 450 | mono | Hashes, addresses, code, log lines |
| `num` | inherits | 600 | display | Any figure: `font-variant-numeric: tabular-nums` |

Rules: no text under 12 px; `label` is the only uppercase style; weight 300
is retired (today's 300-weight 54 px section titles and 26 px tile values
read thin on dark); headings get `text-wrap: balance`; running text 65–72
characters.

### 3.3 Spacing

4 px base. `--space-1` 4, `-2` 8, `-3` 12, `-4` 16, `-5` 24, `-6` 32, `-7` 48,
`-8` 64, `-9` 96. Card padding `-5`; tile padding `-4` `-5`; grid gap `-4`
(phones `-3`); section gap `-9` desktop / `-7` phone; page gutter
`max(16px, 4vw)`; app content max width 1180; docs column 72 ch; wallet column
34 rem.

### 3.4 Radius

`--r-1` 8 (inputs, chips, inline code), `--r-2` 12 (tiles, buttons, table
cards), `--r-3` 20 (cards, dialogs, panels), `--r-pill` 999 (status chips,
the single primary pill on the website hero). Nothing else. The explorer's 4
and 6 px and the chat's five rem values go.

### 3.5 Elevation

Three levels: `flat` (surface + line, the default for every card),
`raised` (surface + `--shadow-1`, hover and selected), `floating` (`--glass`
+ `--shadow-2`, header, popovers, the confirm dialog, the back-to-top
button). One hero card per screen may use `floating` at rest.

### 3.6 Motion

| Token | Value | Use |
|---|---|---|
| `--dur-1` | 120 ms | Hover, focus, chip toggles |
| `--dur-2` | 240 ms | Entrance of a card or row, tab switch |
| `--dur-3` | 480 ms | The one hero entrance per screen, the sent tick |
| `--ease-out` | `cubic-bezier(.22, 1, .36, 1)` | Everything |
| `--ease-spring` | `cubic-bezier(.34, 1.56, .64, 1)` | Buttons and chips on press only |

Permitted loops: one live dot per screen (`pulse`, 2.4 s), the Hub gauge
needle, the 3D hero orbit, the explorer finality pulse. Everything else runs
once from a visible resting state. Under `prefers-reduced-motion: reduce` all
durations become 1 ms (so end states still apply) and the loops stop.

### 3.7 Chart palette

Series in order: `--chart-1` to `--chart-6` (§3.1), the same hue family in
both themes. Each is 3:1 or more on `--bg` and `--surface-1`, and the
closest two stay apart under normal vision, deuteranopia and protanopia
(the generator fails otherwise). Fills at `--chart-fill-alpha` (16 %) of the
stroke. Grid lines `--chart-grid`; axis text `--chart-axis` at `body-sm`; the
emphasised endpoint is a 3 px dot in the series colour with a `--bg` ring.
Status colours (success / warn / danger / info) are never series colours.
Every series is labelled at its end or in a legend, so colour is never the
only key. Empty chart: the plot area keeps its height, draws the grid, and
prints one `body-sm` line in `--text-3` naming why (`No blocks in the window
since 12:41`).

## 4. Component kit (U2)

Shared CSS, documented on `/design-system/` with every component in both
themes. Names are the class names.

| Component | Variants | Notes |
|---|---|---|
| `button` | primary (accent fill, on-accent text), secondary (surface + line), ghost (text only), danger; sizes md 40 px / lg 48 px; icon-only | One primary per screen. `--r-2`; the website hero's primary may be `--r-pill`. |
| `card` | flat (default), raised, floating, hero | Padding `--space-5`, radius `--r-3`, title `title-2`. |
| `stat` | value + label + source; sizes md (28 px value) / lg (40 px); states live / stale / missing | Replaces 6 tile styles. Missing shows `—` and a `body-sm` reason. Value in `num`. |
| `chip` (status) | live, pending, verified, warning, danger, research, neutral; with optional dot | One vocabulary across surfaces: `live · paused · pending · verified · in review · queued · research · planned · shipped`. |
| `table` | default, dense, with sticky head | Card wrapper `--r-2`, row hover `--raised`, numeric columns tabular and right-aligned. |
| `tabs` | underline (docs OS switch), segmented (filters) | 44 px targets; used by `InstallPicker` and Starlight tabs alike. |
| `toast` | info, success, danger | Bottom centre, `floating`, auto-dismiss 6 s, `role="status"`. |
| `empty` / `loading` / `error` states | with icon slot | One shape for "no data", "loading", "unreachable"; never a hero slot. |
| `address` | short (`maya1q8s…k7z2`), full chunked in groups of 4, copy button, QR affordance | Bech32m per ADR-093; the one way an address appears anywhere. |
| `amount` | value + unit; 18-decimal formatting with trailing-zero trimming; size md / lg / hero | `num` style; the unit in `--muted`. |
| `confirm` (money) | clear-sign sentence hero, amount, recipient, fee, details disclosure, warning slot, primary + cancel | The wallet confirm screen (mockup 2); reused by the Hub when it launches the wallet and by chat payment requests. |
| `nav` | website header (collapses to a Menu button under 72 rem), app rail (bottom tab bar under 640 px), drawer | Fixes the 800–1151 px gap. |
| `field` | input, textarea, select, range, checkbox; label, hint, error | Focus ring 2 px `--accent`, offset 2, on every control. |
| `gauge`, `ring`, `sparkline`, `area-chart` | SVG, tokens for colour | Hub and explorer. |
| `wordmark` | emblem + "Maya2C" + product name | `Maya2C` 650 display, product name 500 `--muted`. |

Each component ships with: both themes, keyboard path, `aria` roles,
reduced-motion, a 390 px layout, and a visual baseline in `design/visual`.

## 5. Before → after, per surface

### 5.1 Website: hero and first screen (mockup `mockups/website-hero.html`)

**Before** (`Hero3D.astro`): chip (`offline` when paused) → h1 → lede → three
equal buttons → mono hint → 300 px gap → four tiles → stat band.

**After**, in one screen at 1440 and in 1.4 screens at 390:

- Header: emblem + `Maya2C`; nav `Experience · Build · Run a node · Explorer ↗ · Status ↗`; search; theme. "Built by Eric" moves to the footer.
- Left column: eyebrow `Post-quantum layer-1 · testnet` (a `chip`), h1 **The ledger for the post-quantum century.** (kept), one lede sentence, **one** primary button `Try the testnet` and one text link `How a transaction becomes final →`.
- Right column: the 3D core with the validator ring (kept; poster when WebGL is off), sized to 440 px, not 100 svh.
- Under both: four `stat lg` tiles in the first screen: `Height`, `Block time`, `Validators`, `Finalized tx` from the gateway, each with its source line; `—` plus `paused since 11-10` when the testnet is down. The live chip moves here as the tiles' shared state line.
- Then the facts band (`61 / 166 subsystems verified` etc.) as `stat md` tiles in the same component.
- Removed from the home page: the mono hint, the quest trail (becomes a page), the second ecosystem explorer, the violet counters, the hue-wheel stack diagram (becomes six `--surface` rows with a teal left rule), infinite heading sweeps.
- Phone: headline first at 40 px, the core in a 200 px band under it, one full-width button, tiles 2×2.

### 5.2 Wallet: confirm screen (mockup `mockups/wallet-confirm.html`)

**Before** (`screens.rs:915-969`): seven equal key/value rows, the
clear-sign sentence unstyled, `Sign offline` then `Broadcast now`.

**After**:

- Titlebar: emblem + `Maya2C Wallet`; `chip` `testnet · 241,000` in bronze (mainnet in teal, impostor in danger).
- Title `Confirm transfer`.
- Hero card (the one `floating` card): the core's clear-sign sentence at `title-2` 24 px, verbatim (`Send 25 MAYA to maya1q8s…k7z2 for a fee of 0.001 MAYA on maya-testnet-1`), then `amount hero` `25.000000 MAYA`, then `address full` chunked with copy.
- Rows: `Fee` with the chosen chip (`Economy · Normal · Fast`), `Arrives` (`final in ~2 s`, measured), `From` as `address short`.
- `Details` disclosure, closed: chain id, nonce, signing hash, extra fields from the core.
- Warning slot: `warning` card when fee > amount or the payee label is unverified.
- Actions: one primary `Sign and send` (offline signing stays available as a secondary `Sign only` in the overflow, not as a second primary); `Cancel` ghost.
- Footer line `Signed on this device · ML-DSA-65 + SLH-DSA · keys never leave the OS store` in `body-sm`.
- Phone: identical, 16 px gutter, buttons stacked, primary full width.

### 5.3 Hub: dashboard (mockup `mockups/hub-dashboard.html`)

**Before** (`apps/hub/ui`): gauge hero, eight tiles, two cards, rigs, alerts;
gradient wordmark, mono preview flag, gradient power button, no light theme.

**After**:

- Rail (bottom bar on phones): emblem, `Mining · Network · Wallet · Universe · Settings`, with the active tab in `--accent`.
- Topbar: `wordmark` `Maya2C Hub`; `chip live` `maya-testnet-1 · 241,000`; if preview data, `chip pending` `Preview data`.
- Update banner (new, from the updates directive): `Hub 0.4.1 is ready · signed by the Maya2C release key · Install and restart` as a `card raised` with one primary; dismissible, returns at next launch.
- Hero card: gauge (teal arc, bronze for the over-temperature zone), value `display-2` tabular `5.7 kH/s`, state chip `Mining`, one primary `Stop mining`.
- Six `stat md` tiles: Accepted, Rejected, Earned (on chain), Payout balance, Next share pays, Uptime; epoch progress as a bar inside the Epoch tile.
- Two-column: `Hash rate` area chart (series 1, 18 % fill, endpoint dot) and `Share feed` list with `chip` per state.
- Rigs: one card per device; four `ring` gauges (temp, power, load, fan); a `chip` for the watchdog phase; sparkline; `Start`/`Stop` secondary button.
- Alerts: rows with a level `chip`, message, time.
- Light theme from the tokens; `color-scheme` follows the OS, default dark.

### 5.4 Explorer: dashboard (no mockup; U6 builds from the kit)

- Delete the `tokens.css` and `ui.rs` base layers; `hud.css` becomes the explorer's one stylesheet on the tokens; fonts self-hosted.
- Header `wordmark` `Maya2C Explorer`, nav `Blocks · Transactions · Accounts · Validators · Mining`, search as a `field` with the button inside; theme toggle that works.
- Six `stat md` tiles; the block ribbon stays (cubes in `--surface` with a teal edge; the newest in `--accent`), no hue rotation; finality pulse stays as the one loop.
- Two charts on the chart palette; tables on the `table` component with `address short` and `amount` cells.
- Corner brackets, scan line, spinning arcs, gradient tile values, `▸` prefixes: removed.

### 5.5 Chat: conversation (no mockup; U7)

- Sidebar head: `wordmark` `Maya2C Chat`, `address short` with copy; contacts as rows with avatars; `Add contact` as a `field` pair + secondary button; settings in a drawer.
- Under 760 px the sidebar is a drawer; the conversation is full width with a back button.
- The research notice becomes a `chip warning` beside the peer name, with a popover for the full sentence.
- Bubbles: mine on `--raised` with a teal left edge, theirs on `--surface`; timestamps `body-sm` 12 px minimum; delivery states as `chip` (`sent · delivered · signed receipt · expired`).
- Payment requests (#441) use the `confirm` component in a popover.
- Primary `Send` is `button primary md`; sticker button is `button icon`.

## 6. Open items the implementer must not decide alone

- Website default theme (dark-first today; §8).
- Footer contact lines and social links (§8).
- App names `Maya2C Wallet / Chat / Hub` (§8).
- Whether the explorer keeps a light theme or drops the toggle (recommend keep: it is the only surface a visitor reaches from a search engine in daylight besides the site).

## 7. Acceptance for U1–U9

- `cargo xtask design-tokens --check` passes: every UI's `tokens.css` is byte-identical to the generated one.
- `rg -c "#[0-9a-f]{6}" <ui css>` outside `tokens.css` is 0 on every surface (SVG fills via `currentColor` or CSS variables).
- Every text pair in `tokens.json` has a ratio ≥ 4.5 in both themes, checked by a test in `crates/design-system`.
- Lighthouse on `/`: performance, accessibility, SEO ≥ 95, in the PR.
- axe/pa11y: zero serious issues on the five key screens.
- Visual baselines in `design/visual` for every component in both themes and for each surface's key screen at 390 and 1440.
- Screenshots before/after per surface in the PR; Eric's OK per surface (U9).

## 8. Questions for Eric

1. **Approve this direction** (the one-page artifact linked in the PR shows the mockup shots and before/after).
2. **Website: dark-first or light-first?** Recommendation: keep dark as the brand's home theme and the default for first visits with no OS preference, follow the OS otherwise, remember the choice. All five surfaces get a working light theme either way.
3. **Footer contact (`Footer.astro:26`)**: keep the personal email, or replace it with `security@maya2c.dev` (for disclosures, already implied by the Hall of Fame) plus GitHub, with Facebook/Instagram moved to `eric.maya2c.dev`? Recommendation: replace.
4. **App names**: `Maya2C Wallet`, `Maya2C Chat` (today `Maya Wallet`, `Maya Chat`), matching `Maya2C Hub`? Recommendation: yes, and the installers and store listings follow.
