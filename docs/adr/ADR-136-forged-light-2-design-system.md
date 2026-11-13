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
  (`#08090b`) to deep teal-navy (`#081c28`), with the three surfaces stepped
  up in the same hue. A ground with no hue reads as a screen that is off, and
  forces every accent to be very bright to register; a tinted ground carries
  the brand before any accent is drawn. Panels separate without a hairline:
  `surface-3` against `bg` is gated at 1.25:1 or more in dark (a house rule,
  `Use::Separation`, not WCAG). Accents gain chroma at equal or better
  contrast.
- **Light is warm paper** (`#faf8f4`, warm hairlines and shadows, no inset
  highlight), not a grey inversion of dark.
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
- Two plan values were lifted in lightness only, same hue, because the gate
  measured them below 4.5:1 on `surface-3`: dark `text-3` `#8fa9b3` →
  `#91abb5`, dark `danger` `#ff7183` → `#ff7f8f`. The chart orders given
  per theme were aligned to one hue order, and dark gained a sixth series.
- A seventh generated copy, `apps/status/src/tokens.css`, joins the six;
  the status page does not consume it yet.
- No violet or quantum role is added; the generator refuses those names
  until Eric decides.
