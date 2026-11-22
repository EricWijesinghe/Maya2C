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
role is a tokens.json change with a measured ratio.

### 3.1 Colour

Dark is the brand's home theme and the default on the Tauri apps and the
explorer; the website follows the OS and remembers the visitor's choice
(Eric's decision in §8 can flip the website to light-first; the tokens do not
change).

| Role | Dark | Light | Use |
|---|---|---|---|
| `--bg` | `#050b0f` (abyss, unchanged) | `#f5f8f8` | Page ground. The only place motifs and the 3D field paint. |
| `--surface` | `#0c171d` | `#ffffff` | Cards, panels, tiles, table body, inputs. Solid. |
| `--raised` | `#12232b` | `#e9f0f1` | Hover rows, inline code, chips, second-level cards, inputs on a surface. |
| `--line` | `rgb(150 226 232 / 0.14)` | `rgb(14 116 128 / 0.16)` | Hairlines and card edges. |
| `--text` | `#eef3f3` | `#0b1a1f` | Headlines, values, body on surfaces. |
| `--text-2` | `#b9c6ca` | `#324449` | Running text, ledes, table cells. |
| `--muted` | `#8a9ca3` | `#5a6b71` | Labels, captions, timestamps, sources. 12 px minimum. |
| `--accent` | `#2cc3cf` | `#0a7682` | Primary button fill, focus ring, live dot, selected state, chart series 1. |
| `--accent-text` | `#8ff0f5` | `#085f69` | Links, icon buttons, accent text on any surface. |
| `--on-accent` | `#04171b` | `#ffffff` | Text on `--accent` and on `--bronze`. |
| `--bronze` | `#d9b97a` | `#7d5a22` | Pending, attention, testnet, chart series 2. Non-text and 24 px+ text. |
| `--bronze-text` | `#e8cf98` | `#6b4b16` | Bronze at text sizes. |
| `--success` | `#4fe3a5` | `#0b7a52` | Verified, delivered, confirmed. |
| `--warning` | `#f0bc63` | `#8c5204` | Needs a look; never an error. |
| `--danger` | `#ff7a6b` | `#b4291a` | Errors, failed, impostor network. |
| `--info` | `#8fb4ff` | `#2f55b8` | Research status (the one violet-blue, replacing `#a78bfa`, `#8b7cf6`, `#c4b5fd`). |
| `--glass` | `rgb(12 23 29 / 0.72)` + `blur(16px)` | `rgb(255 255 255 / 0.78)` + `blur(16px)` | Header, popovers, floating buttons only. |
| `--shadow-1` | `0 1px 2px rgb(0 0 0 / .4)` | `0 1px 2px rgb(10 40 46 / .08)` | Contact shadow. |
| `--shadow-2` | `0 24px 60px -28px rgb(0 0 0 / .8)` | `0 24px 60px -30px rgb(10 60 70 / .3)` | Floating chrome and the one hero card per screen. |

Measured contrast of every text pair (WCAG 2.x, `contrast.mjs`):

| Surface | Pair | Foreground | Background | Ratio | WCAG |
|---|---|---|---|---|---|
| dark | text on bg | #eef3f3 | #050b0f | 17.67 | AAA |
| dark | text on surface | #eef3f3 | #0c171d | 16.22 | AAA |
| dark | text on raised | #eef3f3 | #12232b | 14.41 | AAA |
| dark | text-2 on bg | #b9c6ca | #050b0f | 11.3 | AAA |
| dark | text-2 on surface | #b9c6ca | #0c171d | 10.38 | AAA |
| dark | text-2 on raised | #b9c6ca | #12232b | 9.22 | AAA |
| dark | muted on bg | #8a9ca3 | #050b0f | 6.94 | AA |
| dark | muted on surface | #8a9ca3 | #0c171d | 6.37 | AA |
| dark | muted on raised | #8a9ca3 | #12232b | 5.66 | AA |
| dark | teal-text (link) on bg | #8ff0f5 | #050b0f | 15.03 | AAA |
| dark | teal-text on surface | #8ff0f5 | #0c171d | 13.8 | AAA |
| dark | teal-text on raised | #8ff0f5 | #12232b | 12.26 | AAA |
| dark | bronze-text on bg | #e8cf98 | #050b0f | 13.01 | AAA |
| dark | bronze-text on surface | #e8cf98 | #0c171d | 11.95 | AAA |
| dark | success on bg | #4fe3a5 | #050b0f | 12.12 | AAA |
| dark | success on surface | #4fe3a5 | #0c171d | 11.13 | AAA |
| dark | warning on bg | #f0bc63 | #050b0f | 11.38 | AAA |
| dark | warning on surface | #f0bc63 | #0c171d | 10.45 | AAA |
| dark | danger on bg | #ff7a6b | #050b0f | 7.77 | AAA |
| dark | danger on surface | #ff7a6b | #0c171d | 7.14 | AAA |
| dark | info on bg | #8fb4ff | #050b0f | 9.56 | AAA |
| dark | on-accent on teal (primary button) | #04171b | #2cc3cf | 8.58 | AAA |
| dark | on-accent on bronze (secondary emphasis) | #04171b | #d9b97a | 9.77 | AAA |
| dark | teal (non-text: focus ring, icons, 3:1) on bg | #2cc3cf | #050b0f | 9.25 | AAA |
| dark | teal on surface (non-text, 3:1) | #2cc3cf | #0c171d | 8.49 | AAA |
| dark | bronze (non-text, 3:1) on bg | #d9b97a | #050b0f | 10.53 | AAA |
| light | text on bg | #0b1a1f | #f5f8f8 | 16.64 | AAA |
| light | text on surface | #0b1a1f | #ffffff | 17.77 | AAA |
| light | text on raised | #0b1a1f | #e9f0f1 | 15.4 | AAA |
| light | text-2 on bg | #324449 | #f5f8f8 | 9.55 | AAA |
| light | text-2 on surface | #324449 | #ffffff | 10.2 | AAA |
| light | text-2 on raised | #324449 | #e9f0f1 | 8.84 | AAA |
| light | muted on bg | #5a6b71 | #f5f8f8 | 5.21 | AA |
| light | muted on surface | #5a6b71 | #ffffff | 5.56 | AA |
| light | muted on raised | #5a6b71 | #e9f0f1 | 4.82 | AA |
| light | teal-text (link) on bg | #085f69 | #f5f8f8 | 6.9 | AA |
| light | teal-text on surface | #085f69 | #ffffff | 7.37 | AAA |
| light | teal-text on raised | #085f69 | #e9f0f1 | 6.38 | AA |
| light | bronze-text on bg | #6b4b16 | #f5f8f8 | 7.44 | AAA |
| light | bronze-text on surface | #6b4b16 | #ffffff | 7.94 | AAA |
| light | success on bg | #0b7a52 | #f5f8f8 | 5.02 | AA |
| light | success on surface | #0b7a52 | #ffffff | 5.36 | AA |
| light | warning on bg | #8c5204 | #f5f8f8 | 5.92 | AA |
| light | warning on surface | #8c5204 | #ffffff | 6.32 | AA |
| light | danger on bg | #b4291a | #f5f8f8 | 6.01 | AA |
| light | danger on surface | #b4291a | #ffffff | 6.42 | AA |
| light | info on bg | #2f55b8 | #f5f8f8 | 6.32 | AA |
| light | on-accent on teal (primary button) | #ffffff | #0a7682 | 5.35 | AA |
| light | on-accent on bronze (secondary emphasis) | #ffffff | #7d5a22 | 6.25 | AA |
| light | teal (non-text: focus ring, icons, 3:1) on bg | #0a7682 | #f5f8f8 | 5.01 | AA |
| light | teal on surface (non-text, 3:1) | #0a7682 | #ffffff | 5.35 | AA |
| light | bronze (non-text, 3:1) on bg | #7d5a22 | #f5f8f8 | 5.86 | AA |

Every text pair passes AA; dark body pairs pass AAA. `--accent` and
`--bronze` are non-text colours in light mode (5.0 and 5.9 against `--bg`,
more than the 3:1 a focus ring or icon needs); text in those hues uses the
`-text` variants.

Dark values keep the measured Forged Light hues (`#2cc3cf` is `#1fb5c2`
lifted 6 % for the flat button; `#8ff0f5` is `#7eeaf0` with the same lift so
link text clears 7:1 on `--raised`). Light values are the 2026-11-10 AA
corrections, kept.

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

Series in order: `--accent` (teal), `--bronze`, `--text-2` (steel),
`--success`, `--info`. Fills at 18 % alpha of the stroke. Grid lines
`--line`; axis text `--muted` at `body-sm`; the emphasised endpoint is a
3 px dot in the series colour with a `--bg` ring. Status colours (success /
warning / danger) are never series colours. Empty chart: the plot area keeps
its height, draws the grid, and prints one `body-sm` line in `--muted`
naming why (`No blocks in the window since 12:41`).

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
