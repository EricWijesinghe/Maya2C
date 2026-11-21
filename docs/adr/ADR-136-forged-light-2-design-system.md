# ADR-136: Forged Light 2 design system: one token file, generated into every surface

Status: Accepted (UI and tooling only; no consensus, wire or state change)
Date: 2026-11-12

## Context

The website, explorer, Hub, wallet and chat each wrote their own stylesheet
from the Forged Light palette. The November audit
(`docs/design/audit-2026-11.md`) found four token vocabularies for one
palette, about 580 colour literals, five radius scales and, in the explorer,
three style layers. One of those layers was `crates/design-system`'s
`tokens.rs` and its output `design/tokens.css`: a green "calm"/"command"
palette that no other surface used. Eric approved the Forged Light 2
direction (`docs/design/forged-light-2.md`) on 2026-11-12.

## Decision

1. `design/tokens.json` is the single source for colour roles (dark and
   light), type, spacing, the four radii, elevation, motion with its
   reduced-motion values, the chart palette, and the list of
   foreground/background pairs a component may render.
2. `cargo xtask design-tokens` generates one `tokens.css` and writes
   byte-identical copies into `design/` (for the specimen),
   `docs/site/src/styles/`, `apps/explorer/assets/`, `apps/hub/ui/`,
   `apps/chat-gui/ui/` and `apps/wallet-gui/ui/`. Each copy has a
   "do not edit" header with the SHA-256 of its body. `--check` fails when
   a copy differs, and the `ledger` CI job runs it.
3. Dark is the default when the OS states no preference; the OS preference
   applies otherwise; an explicit `data-theme="dark|light"` wins over both.
   Every selector has the specificity of one class, so each surface's own
   stylesheet, loaded after the tokens, still wins where a name is shared
   until U2–U7 move it onto the roles.
4. Both modes refuse a token file in which a listed pair misses WCAG AA
   (4.5:1 for text, 3:1 for non-text), using the design system's existing
   contrast formula. An xtask test prints every ratio.
5. `tokens.rs` and the calm/command palette are deleted, not kept beside
   the new file. The rest of `crates/design-system` (contrast, flows, guard,
   i18n, specimen) stays.

## Consequences

- A colour change is one edit to `tokens.json`, one run of the generator,
  and a CI failure if either step is skipped.
- The explorer's light theme now uses the Forged Light 2 light roles instead
  of calm green; its dark look is unchanged apart from `--danger`.
- The specimen's screenshot baselines are new (`dark-en`, `light-en`,
  `light-ar`) and must be regenerated in the pinned Playwright container.
- Surfaces still carry their own literals; the audit's acceptance check
  (no hex outside `tokens.css`) belongs to U2–U7, not to this change.

## Revision 2026-11-13: developer-tool look

Eric found the first Forged Light 2 tokens "too childish" (large radii,
bright colours, a spring easing). The token file was revised in place to a
quieter developer-tool register; the generator, the drift check and the
contrast gate are unchanged.

- Colour roles: `bg`, `surface-1/2/3`, translucent `border` and
  `border-strong`, `text`, `text-2`, `text-3`, `accent` with `accent-hover`,
  `accent-subtle` and `accent-text`, the primary-button gradient stops,
  `bronze` (sparing), `steel`, `success`, `warn`, `danger`, `glass`, and four
  shadows. `raised`, `line`, `muted`, `info`, `warning` and `bronze-text` are gone.
- Radii are 4/6/8/12 px plus a pill for status dots and chips only. Spacing is
  a 4 px scale named by px / 4. Motion is 120/180/240 ms on one
  `cubic-bezier(.2, 0, 0, 1)`; reduced motion sets the durations to 0 ms.
- Inter (optical-size axis) for everything, JetBrains Mono for code and
  figures; Manrope is removed from the tokens and the site.
- Two measured deviations from the brief's palette, both kept as given and
  handled by role rather than by changing a value: `text-3` measures 3.41 to
  4.10:1 on the dark surfaces, so it is listed as large-text (hints,
  placeholders, disabled) and never body copy; the light `accent` #0e8f80
  measures 3.50 to 3.99:1, so it is a non-text colour (rings, fills, the
  selected tab) and links use the added `accent-text` (#0a7468, 4.96:1 or
  more on every light surface).

## Revision 2 2026-11-13: lifted, tinted palette

Eric found the revision-1 colours "too dark", with "dull feelings". Colour
only; the geometry above (4–12 px radii, 120–240 ms motion, Inter) is kept.
Values: `design/tokens.json`; the table and its reasoning:
`docs/design/forged-light-2.md` §3.1.

- **One meaning per colour.** Teal = trust, secure, the primary action;
  bronze = value, coins, proof, warmth; green = verified or confirmed;
  amber = needs a look, testnet, paused; coral = error or irreversible;
  blue = information or research; steel = reference. A colour that means
  one thing everywhere needs no legend.
- **The dark ground is lifted and tinted** from a hue-free near-black
  (`#08090b`) to deep teal-navy (`#081c28`, the plan's value), with the
  three surfaces stepped up in the same hue. A ground with no hue reads as a screen that is off, and
  forces every accent to be very bright to register; a tinted ground carries
  the brand before any accent is drawn. Panels separate without a hairline:
  a card and `surface-3` against `bg` are gated at 1.25:1 or more in dark (a
  house rule, `Use::Separation`, not WCAG). Accents gain chroma at equal or better
  contrast.
- **Light is warm paper** (`#faf8f4`, warm hairlines and shadows, no inset
  highlight), not a grey inversion of dark; white cards sit on it and
  `surface-3` is gated at 1.25:1 against a card.
- **Roles restored or added:** `bronze-text`, `info`, `border-control`
  (3:1 input edge, SC 1.4.11), the forge stops and `on-forge`, composed
  gradients `grad-cta`, `grad-forge`, `grad-hero`, and a decorative tier
  (`deco-*`, translucent, refused by the gate as a measured role). Chart
  series are per theme (`chart-1`–`chart-6`), declared in each theme block
  because a `var()` resolves where it is declared.
- **`text-3` is body text now**: listed as `text` (4.5:1) on all four
  grounds in both themes, instead of large-text only.
- **The gate grows from pairs to a matrix.** The generator refuses a token
  file that leaves out a required pair (every text role on every ground,
  accent and `border-control` as non-text, ink on every fill), whose chart
  series fall below `chart-distinctness.min-distance` (OKLab) under normal
  vision, deuteranopia or protanopia (Machado et al. 2009), or whose design
  doc table differs from the tokens. `--literals <dir>` lists colour literals
  outside `tokens.css`; U2–U7 drive their folder to zero.
- **Colour-blind rule:** colour is never the only carrier of a state. Every
  status chip carries an icon and a word; every chart series a label.
- The chart orders given per theme were aligned to one hue order, and dark
  gained a sixth series.
- A seventh generated copy, `apps/status/src/tokens.css`, joins the six;
  the status page does not consume it yet.
- No role named violet or quantum is added, and the generator refuses those
  names. `chart-5` (dark `#c58cff`, light `#7c3aed`) is a purple chart
  series, not a role; whether it stays is an open question for Eric
  (forged-light-2.md §8).

### Amended after review (2026-11-14, same revision, before merge)

A design review of the first draft of this revision found the light theme
heavier rather than brighter, and two meanings sharing one colour. Changes:

- **Two tiers per meaning.** One token per role had to pass 4.5:1 as text on
  the light `surface-3`, which forced every light colour to the same dark,
  muddy lightness (OKLCH L 0.43 to 0.51). Each meaning now has a text tier
  (the plain role: `accent-text`, `bronze`, `success`, `warn`, `danger`,
  `info`, `steel`), unchanged in purpose, and a vivid fill tier
  (`bronze-fill`, `success-fill`, `warn-fill`, `danger-fill`, `info-fill`)
  for chip and badge grounds under one dark ink `on-fill` (gated at 4.5:1),
  alert edges and dots. The fills are the same in both themes. In dark this
  also gives `danger` a saturated red fill; the text-safe coral alone read
  soft for "irreversible". The existing names keep their text meaning
  because the wallet and explorer already use `var(--danger)` and
  `var(--warn)` as text colours.
- **Role distinctness is gated.** `role-distinctness` in `tokens.json`
  requires every two roles of the text tier, and of the fill tier with
  `accent`, to be at least `min-distance` (OKLab) apart under normal vision,
  and `bronze`/`warn` and their fills also under deuteranopia and protanopia,
  because no icon-and-word rule separates a coin amount from a warning. The
  first draft had light `bronze` and `warn` 0.021 apart.
- **Hue moved, not only lightness** (superseded below: the values are
  re-derived by lightness and the hue moves are undone). The lane rule was
  "lightness only".
  Separating the roles needed hue: light `bronze` to a deep copper
  (`#782d00`), light `warn` to a yellower amber (`#796300`), light `danger` to
  crimson (`#cb0040`), dark `bronze` to copper-orange (`#ff9c47`), dark
  `steel` greyer (`#a1afb9`). Light `accent` moved from a forest green back
  to the brand teal hue (`#00827d`, the brightest teal that keeps 4.5:1 under
  white ink). This is a deviation from the plan, recorded here.
- **Surfaces** (superseded below). Dark `surface-1` was 1.12:1 against the
  ground; the ladder was `#06202c` / `#0b2d3b` / `#123948` / `#1a4657`, and `text-3`,
  `border-control`, `steel`, `info` and `danger` were lightened to hold their
  minimums on the lifted `surface-3`. Light paper is a little deeper
  (`#f7f3ec`) so a white card reads as paper on paper.
- **Matrix additions:** `border-control` as non-text on `surface-3` (a
  checkbox in a selected row), chart series on `surface-2` (dark `chart-3`
  lightened for it, `#5277ff` since the fourth review, still clear of
  `chart-5` under protanopia),
  `on-fill` on every fill, and the light separation pair.
- **CTA gradient narrowed** (dark `#3fdcd4` to `#19c4bd`, highlight 12%;
  the stops are back to the plan's below, the 12 % highlight stays): the
  first draft read as a glossy gel button.
- `--literals` has no opt-out marker any more; only a
  `<meta name="theme-color">` line is exempt, because the browser reads it
  before any stylesheet.

### Re-derived after the second review (2026-11-14, same revision, before merge)

The second review found the amended palette still off the brief: bronze in
three unrelated colours (copper role, peach forge, orange chart), a muddy
light theme (olive amber, khaki selected row, beige inputs that read as
disabled, three near-black chart series), Bootstrap-like solid chips, dark
steel indistinguishable from `text-3`, and hue moves that broke the
"lightness only" rule without a sign-off. Rather than ask Eric to amend the
rule, every value was re-derived from the brief by OKLCh lightness alone
(chroma clipped to the sRGB gamut where needed), and the gate now enforces
the rule:

- **`hue-families` gate.** Each meaning (teal, bronze, green, amber, coral,
  blue, steel) may span at most 20° of OKLCh hue across its tiers and both
  themes. The brief's own values span up to 19.5° (amber, dark `#ffd43b` to
  light `#855600`). The copper `#782d00` of the first amendment fails it,
  which a test pins.
- **Bronze is one ramp per theme**: role, text, display, solid, forge stops
  and `chart-2`. Where two steps are one colour, one names the other with
  `var()`, and the gate measures through the reference.
- **Bronze and amber at body size** cannot be 0.10 apart at 4.5:1 on paper
  without leaving the bronze hue (the brief's pair is 0.021 apart). Instead
  of moving a hue, amber is never set as bare text: it is a chip or an alert
  with its icon, word and `warn-subtle` ground. The text-tier sets measure
  each against the other meanings; the solids, the tier seen as colour,
  stay a cvd pair (0.136 apart at worst in both themes, by the author's
  calculation; the gate's output is the record).
- **Chips are tinted**, not solid: `*-subtle` ground + 1 px role edge + role
  ink + icon and word, gated as role-on-composite (`warn on warn-subtle over
  surface-2`, and so on). Solids are for dots, alert edges and counters, with
  their own light values and `on-fill` ink per theme.
- **Composited grounds are measured.** `a over b` in a pair is the
  translucent `a` painted over `b`: the selected row (`accent-subtle over
  surface-1`, now teal, not a darker neutral) and the hero's brightest point
  (`deco-teal over surface-1`). The hero glow is tighter and brighter
  (`deco-teal` 0.26 in dark); light has its own warm wash (fourth review,
  below); `text-3` is never set on the hero.
- **`bronze-display`**, a large-text bronze (3:1, 24 px and up), lets a
  balance be bright on paper (`#b56722`) while body bronze keeps 4.5:1.
- **Dark card separation** (`surface-1` against `bg`, 1.25:1) is now gated
  too, not only `surface-3`.
- **Steel** is a blue steel again and must stay 0.04 from `text-2` and
  `text-3` (a set with its own minimum) and 0.10 from the meanings.
- **Charts** may hold at most one ink series per theme (OKLab L under 0.45
  on paper, over 0.92 on the dark ground).
- **One dark block in the stylesheet.** `:root, [data-theme="dark"]` share a
  block and the OS-light block skips a root that chose dark
  (`:root:where(:not([data-theme="dark"]))`, one class of specificity), so
  each copy carries the dark theme once instead of twice. The explorer's
  copy also leaves out the hero, forge, decorative and chart tokens it does
  not draw (`EXPLORER_OMITS` in `xtask/src/design_tokens.rs`); a copy may not
  omit a token one of its own values uses.
- Token values may not fetch: `url(`, `@import` and `image-set(` are
  refused.

Deviation table: every theme value that differs from the 2026-11-13 brief
(the lane plan), and every token the brief did not specify. "L only" means
OKLCh lightness moved with hue held; the "Gate" column names the check that
failed on the brief value.

| Theme | Token | Brief | Shipped | Change | Gate |
|---|---|---|---|---|---|
| dark | `surface-1` | `#0d2734` | `#16303d` | L only | separation, card on `bg`, 1.25 (brief 1.12) |
| dark | `surface-2` | `#133342` | `#1c3c4c` | L only | ladder: the brief's `surface-2` is 1.04 against the lifted card; shipped 1.18 (fourth review: 1.15 left hover rows barely visible) |
| dark | `surface-3` | `#1a4051` | `#22485a` | L only | ladder: the brief's `surface-3` is 1.05 against the shipped `surface-2`; shipped 1.19 above it, 1.78 on `bg`. A wider step (1.21) pushed brief `info` under 4.5 on it |
| dark | `text-3` | `#8fa9b3` | `#9cb7c1` | L only | text on `surface-3`: the brief's value is 3.96 on the shipped `surface-3` (4.48 on the brief's own) |
| dark | `border-strong` | `#6f9aa9` | `#4f7686` | L only | not a gate: the brief's value equalled `border-control`, so a hairline was as loud as an input edge (fourth review); `border-control` keeps `#6f9aa9` |
| both | `warn-ground`, `danger-ground` | not given | dark `#432a08`, `#472020`; light `#fef4df`, `#ffeeec` | added | alert panels: the 12 % tints composite to OKLCh hue 143° (olive) and 300° (plum) over the dark `bg` (`#253124`, `#26222e`); the opaque grounds hold the role hue (70°, 21°); `text`, `text-2` and the role ink 4.5:1 on each |
| both | `disabled-fill`, `on-disabled` | not given | `var(--surface-2)`, `var(--text-3)` | added | a disabled control was the primary at half opacity, still saturated teal with legible ink |
| dark | `danger` | `#ff7183` | `#ff929d` | L only | text on `surface-3` (brief 3.71 on the shipped `surface-3`) |
| dark | `steel` | `#96b8da` | `#accff2` | L only | role distance: steel/info 0.069 in the brief |
| dark | `border-control` | not given | `#6f9aa9` | the brief's `border-strong` | non-text, 3:1 on every ground and a selected row |
| dark | `chart-3` | `#2f6bff` | `#5277ff` | L up, hue +4° (gamut) | non-text on `surface-2` (brief 2.59 on the shipped `surface-2`); lighter than this falls under 0.10 from `chart-5` for protanopes |
| dark | `chart-6` | not given | `#d97f4f` | added | six series in both themes |
| light | `surface-3` | `#ece8e0` | `#e9e5dd` | L only | separation, `surface-3` on a card, 1.25 (brief 1.22) |
| light | `text-3` | `#5f6b69` | `#5d6967` | L only | text on the darker `surface-3` |
| light | `border-control` | `#8d877c` | `#878176` | L only | non-text on `surface-3` (brief 2.92) |
| light | `success` | `#17703a` | `#015728` | L only | role distance: accent/success 0.065 in the brief |
| light | `steel` | `#4a5a70` | `#445469` | L only | role distance: accent/steel 0.097 in the brief |
| light | `chart-3` | `#123f99` | `#2887f3` | L up, clear blue (hue 255°) | ink rule: the brief had three series under L 0.45 (`#06202a`, `#123f99`, `#733f1c`); fourth review: the interim `#056190` was one of four dark series |
| light | `chart-5` | `#7c3aed` | `#5e40a4` | L only (hue 293° held) | chart distance: beside the clear blue, the brief's violet is 0.072 from it for a deuteranope (min 0.10) |
| light | `chart-6` | `#733f1c` | `#9b204c` | replaced (copper to raspberry) | ink rule; fourth review: two browns 0.132 apart read as one; dark keeps copper `#d97f4f` |
| light | `danger-fill` | not given | `#d92d20`, under `on-danger-fill #ffffff` | added | white on the interim `#fe2119` was 3.86 and dark ink on it looked like a sticker; white on `#d92d20` is 4.83, and it clears `fill-vividness` (L 0.576), so no floor exemption is needed |
| both | `bronze-display` | not given | dark `var(--bronze)`, light `#b56722` (the brief's light `chart-2`) | added | large-text bronze, 3:1 on every ground |
| both | `*-subtle` | not given | role solid at 12 % | added | chip ink on its ground, 4.5:1 |
| both | `*-fill`, `on-fill`, `on-bronze-fill` | not given | per theme | added | non-text 3:1 on `bg` and a card; ink 4.5:1; cvd pair; `fill-vividness` |
| light | `forge-grad-from`, `-to` | the dark stops (`#ffd39b`, `#f5a04a`) | `#fbbd6e`, `#e18528` | own light gold ramp, bronze hue | the brief's stops are 1.40 and 2.10 on a card, so the face cannot be the button's boundary; `forge-edge` (`var(--bronze-display)`, 4.26 on a card) is, and `on-forge` is 10.41 and 6.26 on the stops. The interim caramel `#c7802b`/`#bf702e` passed 3:1 but read brown on brown |
| both | `forge-edge`, `forge-highlight` | not given | dark `var(--forge-grad-to)`, light `var(--bronze-display)`; white at 0.2 / 0.5 | added | non-text 3:1 on `bg` and a card (SC 1.4.11) |
| dark | `grad-hero` | not given | tight teal glow | added | text on `deco-teal over surface-1` |
| light | `grad-hero`, `deco-bronze` | not given | white-to-paper wash with a bronze corner, `deco-bronze` at 0.16 | added | the dark recipe at 12 % peaked at `#e2eeed`, a grey smudge on paper; text gated on `deco-bronze over surface-1` |

Every other brief value ships unchanged, among them dark `bg #081c28`,
`text-2 #bcd0d7`, the CTA stops `#52e5dd` to `#0bb5b3`, `bronze #ffbb6b`,
`bronze-text #ffd39b`, `info #6cb6ff`; light `bg #faf8f4`, `accent #0a7568`,
`accent-text #075c52`, CTA `#0a7568` to `#086356`, `bronze #8a5a1f`,
`bronze-text #6f4612` (distinct from `bronze` again), `warn #855600`,
`danger #b3261e`, `info #2f55b8`. Dark `bronze`/`danger` sits well clear of
the threshold now; the role margins the gate prints are the record.

Not decided here: whether `chart-5` stays purple, and whether Manrope
returns as the display face (forged-light-2.md §8).

### Light fills lifted after the third review (2026-11-21, same revision, before merge)

The third review found light still dull: the solid tier carried white ink,
which held every light fill at or under the text tier's depth (success-fill
`#015a2a`, bronze-fill `#6f4612`, warn-fill `#a06b00`), so paper showed no
bright colour. The light fills now carry the dark ink `on-fill #0b1a1f`, as
dark already did, which lets them sit at mid-tone: success `#07a44c`, warn
`#c28203`, danger `#fe2119`, info `#0d7eff`. Bronze is the exception: at the
same mid-tone it lands on amber for deuteranopes and protanopes (0.03 apart,
author's calculation), so light `bronze-fill #a35400` stays darker under its
own white ink `on-bronze-fill` and the cvd pair holds. Hue stays inside each
family's spread. The `fill-vividness` gate (OKLab L and chroma floors in
`tokens.json`) fails a fill that sinks back, and a test pins the old
`#015a2a`. Light `accent` stays the brief's `#0a7568`, because white
`on-accent` needs it; `chart-1 #089192` is the brighter teal for non-text
marks.

### Fourth review (2026-11-21, same revision, before merge)

The design and truth critics found the most safety-critical panels the
dullest, and light bronze brown on brown. Changes, each with the gate that
now holds it (`cargo xtask design-tokens` prints every figure):

- **Alerts get opaque grounds.** `warn-ground` and `danger-ground`, amber-brown
  and red-brown in dark, warm paper tints in light; the specimen's alerts use
  them and the chips keep `*-subtle`. Required pairs: `text`, `text-2` and the
  role ink on each, 4.5:1. A test refuses a translucent value there.
- **The light forge button is gold with a bronze rim.** The face is a ramp
  `#fbbd6e` to `#e18528` under `on-forge`, with a 1 px `forge-highlight`
  top light; `forge-edge` carries the 3:1 boundary, so the gate's solid list
  measures the edge instead of the face. Inside the bronze hue family.
- **The light counter is white on a calmer red** (`on-danger-fill`).
- **Disabled is a neutral step**, not half opacity (`disabled-fill`,
  `on-disabled`); the specimen shows it and a test forbids opacity there.
- **The light hero is a warm wash**, not the dark glow at 12 %; its text is
  gated on `deco-bronze over surface-1` as well as the teal point.
- **The light chart is lighter**: a clear blue, the violet at a lower
  lightness with its hue held, raspberry instead of a second brown. One ink
  series per theme still.
- **The dark ladder is wider** (`surface-2` 1.18 over the card, `surface-3`
  1.19 over `surface-2`), with `text-3`, `danger` and `chart-3` lifted by
  lightness to keep their minimums on the new steps, and `border-strong` is a quieter step than
  `border-control`.

**Brief conflict for Eric: light success and accent.** The brief's light
success `#17703a` is 0.065 from the brief's accent `#0a7568` (min 0.10), so
the shipped success `#015728` is darker and reads close to body text. Moving
the accent bluer does not fix it inside the teal family: at hue 192° the
distance is 0.080, and it only reaches 0.10 near hue 208°, a petrol blue
that then sits 0.095 from `steel`. The two ways out are (a) accept a smaller
accent/success distance in light, because a success chip always carries its
icon and word, or (b) move the light brand teal toward petrol. This lane
ships neither; it is a brand decision (forged-light-2.md §8).
