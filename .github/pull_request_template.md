<!--
Pull requests are suggestions: the maintainer inspects each one and decides.
Only the maintainer merges. CI on a pull request from a fork waits for the
maintainer's approval before it runs. See CONTRIBUTING.md.
-->

## What and why

<!-- One paragraph. Link the issue or Discussion. -->

## How it was tested

<!-- Paste the real command output. A claim without output is not a result. -->

## Checklist

- [ ] A test fails without this change and passes with it (or: docs only)
- [ ] `features.toml` updated if a subsystem's behaviour or status changed, naming real tests
- [ ] Docs updated, and the site page under `docs/site/` follows any user-visible change
- [ ] `cargo xtask claims-check` passes: no "first", "only" or "unprecedented" without a dated `docs/prior-art/` record
- [ ] Numbers in this PR were measured, with the command beside them
- [ ] One cargo scope at a time: built and tested with `-p <crate>`, or marked "Not compiled locally; CI is the first compile"

## Risk

- [ ] Touches consensus, state roots, fees, the wire format or cryptography (needs spec + vectors, an ADR if behaviour changes, and two human reviewers; read `docs/invariants.md`)
- [ ] Adds or updates a dependency
- [ ] No secrets, keys, seed phrases or personal data in this change
