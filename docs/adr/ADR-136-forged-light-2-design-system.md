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
