# ADR-075: Node "update available" notice, notify only

**Status:** Proposed (2026-11-01). Backlog B4. Nothing here installs anything.
**Date:** 2026-11-01

## Context

Standing directive (2026-10-30, `owner-directive-updates-security`): installed
software must be able to learn that a newer release exists. The Hub and the
wallet/chat apps already have a signed in-app updater (`docs/in-app-updates.md`,
`apps/app-update`, merged via #258). The node binary (`maya2c-node`) has none.

Facts that bound the decision:

- The node is a server process run by operators, not an end-user app. A
  silent binary swap on a validator is a consensus risk (a node that restarts
  onto an unreviewed build can change state roots), so installation must not
  happen without an operator.
- The shared updater signs Tauri bundles with the updater key. Those are not
  node binaries, so the node cannot reuse its install path.
- `maya2c-node` has no HTTP client. The workspace has `hyper` (no TLS) and no
  `reqwest`/`ureq` in the node's dependency set. A manifest fetch over HTTPS
  needs one, and a new dependency needs a `cargo vet` / `cargo deny` entry
  (CLAUDE.md, Production Standing Orders).

## Decision

1. **Notify only.** The node may tell the operator that a newer version is
   published. It never downloads, verifies-and-installs, or restarts.
2. **Same manifest as the apps.** The notice reads `updates/<app>.json` from
   the site, the file `docs/site/scripts/gen-updates.mjs` already writes, so
   there is one release source. The node entry is `maya2c-node.json`.
3. **Off by default.** A node makes an outbound request only when started with
   an explicit `--check-updates` flag. A validator on a private network does
   not phone home unless the operator asks.
4. **The decision is pure and tested.** Parsing the manifest and comparing
   versions (`0.1.N`, numeric) is a function with no I/O, so it is tested
   without a network. The fetch is a separate, thin step.
5. **Shipped in this change:** the pure comparison (`update_notice`) and its
   tests. **Not shipped:** the fetch, the `--check-updates` flag, and any HTTP
   dependency. Those wait for a dependency decision (see Revisit when).

## Alternatives considered

- **Reuse `apps/app-update` for the node.** Rejected: it installs, and it is
  built for Tauri bundles. A notice must not install.
- **Auto-install.** Rejected for a validator: an unreviewed binary can change
  consensus. Operators install by hand after reading the notice.
- **Verify the manifest signature.** Deferred, not rejected. The manifest is
  served over HTTPS and is only a notice in this step: it names a version and
  a page, and cannot make the node run code. Verifying it becomes necessary
  the moment a notice carries a download URL the node acts on.

## Consequences

- An operator learns of a release only when the node is started with
  `--check-updates` and the fetch step lands. Until then the notice is library
  code with no caller.
- Notify-only means a compromised manifest could show a false notice. The cost
  is a wasted check, not a changed binary.
- A new HTTP client dependency will need `cargo vet` and `cargo deny` review.

## Revisit when

- The fetch step is written: decide the HTTP client (rustls-based, vetted) and
  add a `cargo vet` entry in the same change.
- Any notice starts carrying a download URL or a hash the node acts on: then
  the manifest must be signed and verified (the shared updater key or a
  node-specific key, decided in a new ADR).
