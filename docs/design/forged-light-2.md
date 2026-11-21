# Forged Light 2: one design system for every Maya2C surface

Status: **accepted** (Eric approved the direction on 2026-11-12, recorded in
[ADR-136](../adr/ADR-136-forged-light-2-design-system.md); plan
`ui-greatest-upgrade`). The tokens (§3.1, §3.7) are implemented in
`design/tokens.json`; the component kit and the surface rebuilds (§4, §5) are
not yet. The evidence is in
[audit-2026-11.md](audit-2026-11.md); the mockups are in
[mockups/](mockups/) with screenshots in [shots/](shots/).

Forged Light (2026-10-05) chose the right materials: the emblem's teal,
bronze and steel on an abyss ground, Manrope for display, Inter for reading,
JetBrains Mono for data (the display face is open again, §8 item 6). Five surfaces then each wrote their own stylesheet
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
5. **Meaning decides colour.** Teal means trust and the primary action,
   bronze means value, green means verified, amber means needs a look, coral
   means error, blue means information or research, steel means reference
   (the table in §3.1). Position (`nth-child`), category and decoration never
   pick a colour.
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
| Teal / bronze / steel / abyss, from the emblem | Named roles (§3.1) replace ~580 colour literals; Tailwind reds, greens, violets and the explorer's cyan/indigo and calm/command green are deleted |
| Self-hosted Inter + JetBrains Mono (Manrope for display: open, §8 item 6) | The explorer loads them (today it names Inter and never loads it); mono is reserved for hashes, addresses, code and tabular figures |
| "Maya2C" one word | App names become `Maya2C Hub`, `Maya2C Wallet`, `Maya2C Chat` (decision for Eric) |
| Real numbers, `—` when missing | A missing number also gets a one-line reason (`testnet paused`, with the date the status source reports; `explorer unreachable`), in muted text, never in the hero slot |
| Graphics over text: 3D hero, orbs, journey, map, gauge | Orb rows only for 4 or 6 items; longer lists are tile grids; one chart palette; the stack diagram drops the hue wheel |
| Dark as the brand's home theme | Light is a first-class theme on all five surfaces, including the Tauri apps and the explorer (its toggle works or goes) |
| Soft geometry, glass with depth | Four radii instead of fifteen; glass only on floating chrome (header, popovers); cards are solid surfaces so text never sits on blur |
| Short motion on interaction (120–240 ms, one easing, no spring) | Infinite sweeps on rules and dividers, the explorer scan line and arcs, and scroll-driven reveals are removed |
| Reduced-motion honoured | One technique everywhere: durations to 1 ms (the wallet's), not `animation: none` (which breaks `animation-fill-mode: both` end states) |
| Starlight for docs | Reading pages get a plain background; motifs only on home, status and experience |

## 3. Tokens

The source is `design/tokens.json`, generated into `tokens.css` in every
surface by `cargo xtask design-tokens`, with `--check` in CI as the drift
gate. The old `crates/design-system` palette (calm/command, green) is gone;
the crate now holds the contrast maths and the specimen.

Names are role names. No surface may add a colour outside this list; a new
role is a tokens.json change with a measured ratio. `cargo xtask
design-tokens --literals <dir>` lists every colour a folder still writes by
hand.

Every section of §3 describes the token file as it is (revision 2,
2026-11-13, values re-derived on 2026-11-14). Where this text and
`design/tokens.json` ever disagree, the token file wins, and for §3.1 the
generator refuses the disagreement.

### 3.1 Colour

Dark is the brand's home theme and the default when the OS states no
preference (Eric, 2026-11-12); the OS preference applies otherwise, and an
explicit `data-theme` wins over both. The values below are
`design/tokens.json`. `cargo xtask design-tokens --check` compares this table
with the token file and fails on any difference, so the table cannot drift
from what ships.

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

The rule is gated, not only written. `role-distinctness` requires every two
meaning roles of a set to be at least 0.10 apart in OKLab under normal
vision, in both themes; steel must also stay 0.04 from `text-2` and `text-3`,
so reference figures do not read as captions. The solid fills of bronze and
amber are also checked under deuteranopia and protanopia, because no
icon-and-word rule separates a coin amount from a warning.

One pair is separated by a rule instead of by colour: body-size bronze and
body-size amber. At 4.5:1 on paper both must be dark, and the brief's light
bronze `#8a5a1f` and amber `#855600` are 0.021 apart, the same brown. Moving
bronze to copper (the first draft of this revision) breaks the hue it shares
with the emblem; moving amber far enough turns it olive. So amber is never
set as bare text: it always appears as a chip or an alert, with its icon,
its word and its `warn-subtle` ground, and the text-tier sets measure bronze
and amber each against the other meanings, not against each other.

**Tiers per meaning.** A colour dark enough for body text on paper is too
dark to look bright, so each meaning has tiers, each with one job:

| Tier | Tokens | Job |
|---|---|---|
| Text | `accent-text`, `bronze`, `bronze-text`, `success`, `warn`, `danger`, `info`, `steel` | Body-size text on every surface, a chip's word and edge. 4.5:1. |
| Chip ground | `bronze-subtle`, `success-subtle`, `warn-subtle`, `danger-subtle`, `info-subtle`, `accent-subtle` | A translucent tint under the text tier: tint + 1 px role edge + role ink + icon and word. |
| Solid | `bronze-fill`, `success-fill`, `warn-fill`, `danger-fill`, `info-fill` | Dots, alert edges and counters under `on-fill` only. 3:1 on the ground and a card. Never a chip ground. |
| Display | `bronze-display` | Bronze for figures of 24 px and up (3:1, large text), so a balance can be bright on paper. |

**Bronze is one ramp.** Each theme's bronze steps are the role, the text
tone, the display tone, the solid, the two forge stops and chart series 2;
where two of them are the same colour, one names the other with `var()`, and
the gate measures the step it names. The `hue-families` gate holds every
meaning (teal, bronze, green, amber, coral, blue, steel) to a 20° OKLCh hue
spread across its tiers and both themes, so a value that fails a contrast
pair is fixed by lightness, not by drifting to a neighbouring hue.

**Why the ground was lifted and tinted.** Revision 1 used a hue-free near-black
(`#08090b`) with grey text and a muted teal. Eric's reading was "too dark,
dull". A ground with no hue reads as a switched-off screen, and on a
near-black every accent has to be very bright to register, which is where the
dullness came from: little contrast between the ground and the cards, and
colour that looked dimmed. The ground is now a deep teal-navy (`#081c28`), so
the brand hue is present before any accent is drawn; a card is lifted 1.25:1
off it and the pressed `surface-3` further, both gated, so a panel separates
without a hairline; and the hero is a tight teal glow from the top rather than
a wide wash. Light is warm paper (`#faf8f4`) with warm hairlines and shadows,
not a grey inversion of dark. On paper every input and secondary button is a
white `surface-1` with a `border-control` edge, never a beige fill (which
reads as disabled), and a selected row is the teal `accent-subtle` over a
card, not a darker beige.

**Colour-blind rule.** Colour is never the only carrier of a state. Every
status chip shows an icon and a word ("✓ Verified", "! Testnet paused",
"✕ Failed"), a value carries its unit, and a chart series has a label at its
end or in a legend. The chart series below are additionally checked: the
generator simulates deuteranopia and protanopia (Machado, Oliveira and
Fernandes 2009, severity 1.0) and fails when the two closest series fall
below `chart-distinctness.min-distance` (OKLab) in either theme. Status
colours and fills are never series colours.

| Token | Dark | Light | Use |
|---|---|---|---|
| `--bg` | `#081c28` | `#faf8f4` | Page ground. The only place motifs and the 3D field paint. |
| `--surface-1` | `#16303d` | `#ffffff` | Cards, panels, table body; every input and secondary button. 1.25:1 against `bg` in dark. |
| `--surface-2` | `#1a3a49` | `#f4f1eb` | Hover row, inline code. |
| `--surface-3` | `#1e4455` | `#e9e5dd` | Pressed. 1.25:1 or more against `bg` (dark) and a card (light). |
| `--border` | `rgb(150 210 225 / 0.14)` | `rgb(60 45 20 / 0.14)` | Hairlines and card edges. Not a control edge. |
| `--border-strong` | `#6f9aa9` | `rgb(60 45 20 / 0.26)` | Dividers that must be seen, table header rule. |
| `--border-control` | `#6f9aa9` | `#878176` | Input, checkbox and select edges: 3:1 on all four grounds and a selected row (SC 1.4.11). |
| `--text` | `#f2f9fa` | `#14201f` | Headlines, values, body. |
| `--text-2` | `#bcd0d7` | `#3d4a49` | Running text, ledes, table cells. |
| `--text-3` | `#96b1bb` | `#5d6967` | Labels, captions, timestamps, sources. Body-size text: 4.5:1 on every surface. Never on the hero. |
| `--accent` | `#1fd3cc` | `#0a7568` | Focus ring, live dot, checked box, icons. Non-text. |
| `--accent-hover` | `#52e5dd` | `#075c52` | Hover of an accent control. |
| `--accent-subtle` | `rgb(31 211 204 / 0.14)` | `rgb(10 117 104 / 0.1)` | Selected row, accent chip ground. |
| `--accent-text` | `#52e5dd` | `#075c52` | Links and accent labels at any size. |
| `--accent-grad-from` | `#52e5dd` | `#0a7568` | Primary button, top stop (`--grad-cta`). |
| `--accent-grad-to` | `#0bb5b3` | `#086356` | Primary button, bottom stop. |
| `--accent-highlight` | `rgb(255 255 255 / 0.12)` | `rgb(255 255 255 / 0.14)` | The 1 px inner top edge of the primary button. |
| `--on-accent` | `#04222a` | `#ffffff` | Text on `--accent`, `--accent-hover` and the CTA stops. |
| `--bronze` | `#ffbb6b` | `#8a5a1f` | Value: amounts, coins, proofs at body size. |
| `--bronze-text` | `#ffd39b` | `#6f4612` | Bronze for long runs of text; the deepest step. |
| `--bronze-display` | `var(--bronze)` | `#b56722` | Bronze figures of 24 px and up: 3:1 on every surface. |
| `--steel` | `#accff2` | `#445469` | Reference figures: hashes, heights, IDs. |
| `--success` | `#4ade80` | `#015728` | Verified, confirmed, delivered. Text tier. |
| `--warn` | `#ffd43b` | `#855600` | Needs a look: testnet, paused, pending. Never an error, never bare text. |
| `--danger` | `#ff8994` | `#b3261e` | Error, failed, irreversible. Text tier. |
| `--info` | `#6cb6ff` | `#2f55b8` | Information, research status. Text tier. |
| `--bronze-subtle` | `rgb(233 138 19 / 0.12)` | `rgb(181 103 34 / 0.12)` | Value chip ground. |
| `--success-subtle` | `rgb(60 207 110 / 0.12)` | `rgb(34 163 90 / 0.12)` | Verified chip ground. |
| `--warn-subtle` | `rgb(248 202 6 / 0.12)` | `rgb(255 198 26 / 0.12)` | Needs-a-look chip and alert ground. |
| `--danger-subtle` | `rgb(255 77 90 / 0.12)` | `rgb(229 72 77 / 0.12)` | Error chip and alert ground. |
| `--info-subtle` | `rgb(79 155 255 / 0.12)` | `rgb(59 130 246 / 0.12)` | Information chip ground. |
| `--bronze-fill` | `#e98a13` | `var(--bronze-text)` | Coin dot, value counter. Solid tier, under `--on-fill`. |
| `--success-fill` | `#3ccf6e` | `#015a2a` | Verified dot, alert edge, counter. |
| `--warn-fill` | `#f8ca06` | `#a06b00` | Needs-a-look dot, alert edge, counter. |
| `--danger-fill` | `#ff4d5a` | `#ec0209` | Error dot, alert edge, counter. |
| `--info-fill` | `#4f9bff` | `#1e4fff` | Information dot, alert edge, counter. |
| `--on-fill` | `#0b1a1f` | `#ffffff` | Ink on every `*-fill`: 4.5:1 on each. |
| `--forge-grad-from` | `var(--bronze-text)` | `#c7802b` | Forge button (value actions: sign, stake), top stop (`--grad-forge`). |
| `--forge-grad-to` | `#f5a04a` | `#bf702e` | Forge button, bottom stop. Both stops 3:1 on the ground and a card. |
| `--on-forge` | `#2b1503` | `#2b1503` | Text on the forge stops. |
| `--deco-teal` | `rgb(31 211 204 / 0.26)` | `rgb(0 200 190 / 0.24)` | Decorative only: the hero glow and the page wash. Light is its own luminous aqua, not dark's teal dimmed. Text over its brightest point is gated. |
| `--deco-bronze` | `rgb(255 187 107 / 0.16)` | `rgb(255 168 56 / 0.22)` | Decorative only: backdrops and the page wash (warm amber in light). |
| `--deco-blue` | `rgb(108 182 255 / 0.12)` | `rgb(47 85 184 / 0.08)` | Decorative only. |
| `--deco-spark` | `#52e5dd` | `#00c2b2` | The one solid decorative accent: the forged rule, the active-nav underline, chart highlights. Never text, never the only cue, never a focus ring (focus stays `--accent`, 3:1). |
| `--glass` | `rgb(22 48 61 / 0.8)` | `rgb(250 248 244 / 0.82)` | Header and command palette only, with `--glass-blur`. |
| `--shadow-sm` | `0 1px 2px rgb(2 10 16 / 0.45)` | `0 1px 2px rgb(60 45 20 / 0.08)` | Contact shadow. |
| `--shadow-md` | `0 1px 2px rgb(2 10 16 / 0.45), 0 8px 24px -8px rgb(2 10 16 / 0.55)` | `0 1px 2px rgb(60 45 20 / 0.08), 0 8px 24px -8px rgb(60 45 20 / 0.14)` | Raised: hover and selected. |
| `--shadow-lg` | `0 2px 4px rgb(2 10 16 / 0.45), 0 24px 48px -12px rgb(2 10 16 / 0.65)` | `0 2px 4px rgb(60 45 20 / 0.1), 0 24px 48px -12px rgb(60 45 20 / 0.18)` | Floating: header, palette. |
| `--shadow-inset` | `inset 0 1px 0 rgb(255 255 255 / 0.06)` | `inset 0 0 0 0 transparent` | Top-edge light on dark cards; none on paper (written as a zero shadow so it still composes in a `box-shadow` list). |
| `--chart-1` | `#1fd3cc` | `#089192` | Series 1, teal. |
| `--chart-2` | `var(--bronze)` | `var(--bronze-display)` | Series 2, bronze (the ramp's bright step). |
| `--chart-3` | `#4e74ff` | `#056190` | Series 3, blue (§3.7). |
| `--chart-4` | `#f2f9fa` | `#06202a` | Series 4, neutral ink: the one ink series per theme. |
| `--chart-5` | `#c58cff` | `#7c3aed` | Series 5, purple: a chart series, not a role. Unused until Eric decides (§8). |
| `--chart-6` | `#d97f4f` | `#8b3f03` | Series 6, copper. |

Composed tokens, declared in each theme block because a `var()` resolves where
it is declared (a nested `data-theme` must re-declare them): `--grad-cta`
(the two accent stops, top to bottom), `--grad-forge` (the two forge stops),
`--grad-hero` (a tight `--deco-teal` glow from the top centre over
`--surface-1` fading to `--bg`; no bronze wash), `--chart-grid` (`--border`)
and `--chart-axis` (`--text-3`).

Every measured pair is listed in `contrast-pairs` and checked in both themes
on every generator run, which prints each ratio. A background written
`a over b` is measured as a browser paints it: the translucent `a`
composited over `b`. The generator refuses a file that leaves out a pair of
its required matrix: every text role as body text (4.5:1) on the four grounds
and on a selected row; `bronze-display` as large text (3:1) there and on the
hero; `text`, `text-2`, `accent-text` and `bronze-text` on the hero's
brightest point; each chip's role on its own `*-subtle` ground over `bg`,
`surface-1` and `surface-2`; `accent` and `border-control` as non-text on the
four grounds and a selected row; `on-accent`, `on-forge` and `on-fill` on
their grounds; every solid and both forge stops as non-text on `bg` and a
card; every chart series on `bg`, `surface-1` and `surface-2`; and three
separation pairs (dark card and dark `surface-3` against `bg`, light
`surface-3` against a card). Ratios are not copied here: the gate's output is
the record, and the PR that changes a value pastes it.

Where the values differ from the 2026-11-13 brief, each row is in ADR-136
(second revision, deviation table) with the gate that forced it. In short:
lightness only, same hue, wherever a pair failed (dark `surface-1`/`-2`/`-3`,
`text-3`, `danger`, `steel`, `chart-3`; light `surface-3`, `text-3`,
`border-control`, `success`, `steel`), new tokens the brief did not have
(`bronze-display`, the `*-subtle` chip grounds, per-theme solids), and two
chart series on paper replaced because three of the brief's six were
near-black (`chart-3`, `chart-6`). No role named violet or quantum exists;
whether `chart-5` keeps its purple hue is Eric's decision (§8).

### 3.2 Type

Self-hosted variable fonts on every surface. Today the token file names one
reading face for everything (`--font-sans`, Inter) and JetBrains Mono for
data; whether Manrope returns as the display face is open (§8, item 6), and
no surface lane picks a display font until Eric answers.

| Token | Value |
|---|---|
| `--font-sans` | `"Inter Variable", "Inter", system-ui, -apple-system, "Segoe UI", sans-serif` |
| `--font-mono` | `"JetBrains Mono Variable", "JetBrains Mono", ui-monospace, Consolas, monospace` |
| `--font-numeric` | `tabular-nums` |

One scale for every surface (px), weights 400 / 500 / 600 only, body line
1.5, headings 1.2, display sizes tracked -0.02em:

| Step | Size, phones | Weight | Used for |
|---|---|---|---|
| `type-display-1` | 56, 40 | 600 | Website hero h1 only |
| `type-display-2` | 40, 32 | 600 | Section titles, wallet hero amount |
| `type-title-1` | 32, 24 | 600 | Page titles |
| `type-title-2` | 24 | 600 | Card titles, the clear-sign sentence |
| `type-title-3` | 20 | 600 | Sub-heads |
| `type-body-lg` | 16 | 400 | Documentation body |
| `type-body` | 14 | 400 | App body, site UI copy |
| `type-body-sm` | 13 | 400 | Hints, sources, table cells |
| `type-label` | 12 | 500 | Labels |
| `type-mono` | 13 | 400 | Hashes, addresses, code, log lines |

Rules: no text under 12 px; headings get `text-wrap: balance`; running text
65–72 characters; any figure uses `--font-numeric`.

### 3.3 Spacing

4 px base; the step number is px / 4: `--space-1` 4, `-2` 8, `-3` 12, `-4`
16, `-5` 20, `-6` 24, `-8` 32, `-10` 40, `-12` 48, `-16` 64, `-20` 80, `-24`
96. Page gutter `max(16px, 4vw)` (16 px at phone width); content max width
1180 px; docs column 72 ch; wallet column 34 rem; controls 34 px high
(`--control-h`), 28 px small (`--control-h-sm`).

### 3.4 Radius

`--r-xs` 4, `--r-sm` 6, `--r-md` 8 (buttons, inputs), `--r-lg` 12 (cards,
panels, dialogs). Nothing above 12. `--r-pill` 999 is for status dots and
chips only, never a button or a card.

### 3.5 Elevation

Three levels: `flat` (surface + `--border`, every card), `raised`
(`--shadow-md` + `--shadow-inset`, hover and selected), `floating`
(`--glass` + `--glass-blur` + `--shadow-lg`, the header and the command
palette only). Text never sits on blur.

### 3.6 Motion

| Token | Value | Use |
|---|---|---|
| `--dur-1` | 120 ms | Hover, focus, chip toggles |
| `--dur-2` | 180 ms | Entrance of a card or row, tab switch |
| `--dur-3` | 240 ms | The one hero entrance per screen, the sent tick |
| `--ease` | `cubic-bezier(.2, 0, 0, 1)` | Everything: short, no spring, no overshoot |

Permitted loops: one live dot per screen, the Hub gauge needle, the 3D hero
orbit, the explorer finality pulse. Everything else runs once from a visible
resting state. Under `prefers-reduced-motion: reduce` every duration becomes
0 ms, so end states still apply, and the loops stop.

### 3.7 Chart palette

Series in order: `--chart-1` to `--chart-6` (§3.1), the same hue family in
both themes. A chart is drawn on `--bg`, `--surface-1` or `--surface-2`,
never on the pressed or selected `--surface-3`; each series is 3:1 or more on
those three, the closest two stay apart under normal vision, deuteranopia and
protanopia, and at most one series per theme is an ink series near the text
colour (OKLab L below 0.45 on paper, above 0.92 on the dark ground); the
generator fails otherwise. Fills at `--chart-fill-alpha` (16 %) of the
stroke. Grid lines `--chart-grid`; axis text `--chart-axis` at
`type-body-sm`; the emphasised endpoint is a 3 px dot in the series colour
with a `--bg` ring. Status colours and their fills (success / warn / danger /
info) are never series colours. `--chart-3` is blue, the family of `--info`,
by exception: a series never appears as a chip, and `info` always carries its
icon and word, so the two are not read for each other. `--chart-5` is not
drawn on any surface until Eric decides its hue.
Every series is labelled at its end or in a legend, so colour is never the
only key. Empty chart: the plot area keeps its height, draws the grid, and
prints one `type-body-sm` line in `--text-3` naming why (`No blocks in the
window since 12:41`).

## 4. Component kit (U2)

Shared CSS, documented on `/design-system/` with every component in both
themes. Names are the class names.

| Component | Variants | Notes |
|---|---|---|
| `button` | primary (accent fill, on-accent text), secondary (surface + line), ghost (text only), danger; heights `--control-h` 34 px / `--control-h-sm` 28 px; icon-only; hover, focus and disabled states | One primary per screen. `--r-md`, never a pill. Secondary is `surface-1` + `border-control`. |
| `card` | flat (default), raised, floating, hero | Padding `--space-5`, radius `--r-lg`, title `type-title-2`. |
| `stat` | value + label + source; sizes md (28 px value) / lg (40 px); states live / stale / missing | Replaces 6 tile styles. Missing shows `—` and a `body-sm` reason. Value in `num`. |
| `chip` (status) | live, pending, verified, warning, danger, research, neutral; with optional dot | One vocabulary across surfaces: `live · paused · pending · verified · in review · queued · research · planned · shipped`. |
| `table` | default, dense, with sticky head | Card wrapper `--r-lg`, row hover `--raised`, numeric columns tabular and right-aligned. |
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

Every figure in this section and in the mockups (heights, amounts, rates,
versions) is an illustrative placeholder that shows a layout, not a value. On
a surface each one comes from its live source, or shows `—` with a one-line
reason. Mainnet is not live and the testnet is paused, so no screen states or
implies a running network while that holds; a state chip reads what the
status source reports.

### 5.1 Website: hero and first screen (mockup `mockups/website-hero.html`)

**Before** (`Hero3D.astro`): chip (`offline` when paused) → h1 → lede → three
equal buttons → mono hint → 300 px gap → four tiles → stat band.

**After**, in one screen at 1440 and in 1.4 screens at 390:

- Header: emblem + `Maya2C`; nav `Experience · Build · Run a node · Explorer ↗ · Status ↗`; search; theme. "Built by Eric" moves to the footer.
- Left column: eyebrow `Post-quantum layer-1 · testnet` (a `chip`), h1 **The ledger for the post-quantum century.** (kept), one lede sentence, **one** primary button and one text link `How a transaction becomes final →`. The primary follows the network state: while the testnet is paused it is `Read the docs`, never `Try the testnet`; `Try the testnet` returns only when the status source reports it running.
- Right column: the 3D core with the validator ring (kept; poster when WebGL is off), sized to 440 px, not 100 svh.
- Under both: four `stat lg` tiles in the first screen: `Height`, `Block time`, `Validators`, `Finalized tx` from the gateway, each with its source line; `—` plus `testnet paused` and the pause date read from the status source (never typed into the page) when the testnet is down, which it is today. The state chip moves here as the tiles' shared state line.
- Then the facts band (`<verified> / <subsystems> subsystems verified` etc.) as `stat md` tiles in the same component, both counts generated from `features.toml` at build time, never hand-typed.
- Removed from the home page: the mono hint, the quest trail (becomes a page), the second ecosystem explorer, the violet counters, the hue-wheel stack diagram (becomes six `--surface` rows with a teal left rule), infinite heading sweeps.
- Phone: headline first at 40 px, the core in a 200 px band under it, one full-width button, tiles 2×2.

### 5.2 Wallet: confirm screen (mockup `mockups/wallet-confirm.html`)

**Before** (`screens.rs:915-969`): seven equal key/value rows, the
clear-sign sentence unstyled, `Sign offline` then `Broadcast now`.

**After**:

- Titlebar: emblem + `Maya2C Wallet`; `chip` with the network name and the height the node reports (`testnet · —` while the node has no height), in `warn` with its icon for testnet (mainnet in teal, impostor in danger).
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
- Topbar: `wordmark` `Maya2C Hub`; a state `chip` with the chain id and the height from the gateway (`maya-testnet-1 · paused · —` today; `live` only when the gateway reports it); if preview data, `chip pending` `Preview data`.
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
5. **Chart series 5 (open, 2026-11-14).** `--chart-5` is purple (dark `#c58cff`, light `#7c3aed`). It is a chart series, not a role, and no role named violet or quantum exists. Keep it, or replace it with a non-purple series hue? Recommendation: keep until a chart needs more than four series, then decide with the real chart in front of you. Until then no surface draws it.
6. **Manrope (open, 2026-11-14).** The brand rule names Manrope + Inter, self-hosted; the token file (revision 1 of ADR-136) says Inter for everything and Manrope retired. Which is the rule? This lane changed colour only and left the fonts as they are.
