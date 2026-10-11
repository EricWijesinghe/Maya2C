# 2026-10-12 — GitHub repository hardening

Goal (Eric): contributions are suggestions only; maximum protection; keep
control with the owner; open for discussion, reports and donations.

## Applied (GitHub settings, verified by API read-back)

| Setting | Before | After |
|---|---|---|
| `master` protection | none (`Branch not protected`, HTTP 404) | ruleset 24884482: PR required, 6 required checks, no force-push/delete, admin bypass via PR only |
| Tags | unprotected | ruleset 24884483: create/update/delete admin only |
| Fork PR CI | approval for first-time contributors | approval for all external contributors |
| Allowed actions | all, unpinned | GitHub-owned + 7 named owners, `sha_pinning_required: true` |
| Discussions / Wiki / Projects | off / on / on | on / off / off |
| Private vulnerability reporting | off | on |
| Web commit sign-off | off | required |
| Auto-delete merged branches | off | on |
| Remote branches | master, attacknet-reports, 2 stale fix branches | master, attacknet-reports (stale ones were already in master: `git cherry` showed `-` / merge-only) |
| Releases | testnet.1, .2, .3 | testnet.3 only |

Not available: secret-scanning validity checks and non-provider patterns
(read back `disabled` after PATCH — paid Secret Protection on personal accounts).

## CodeQL

865 open → 862 dismissed with per-alert reasons (`dismissed=862 failed=0`),
3 JS fixed in PR #109. PR #109's CodeQL check caught an incomplete first fix
of `js/bad-tag-filter` (`</script\s*>` → `</script[^>]*>`); corrected.

## Merged

- PR #109 `55bf1e7d` — CODEOWNERS, CONTRIBUTING, CODE_OF_CONDUCT, issue forms, PR template, JS fixes. 27 pass, 1 skipping.
- PR #110 `02aa9456` — 108 actions pinned to SHAs; site link to the deleted testnet.1 release (caught by `site-links`) now points at the releases list. 32 pass, 2 skipping, all six required checks pass.

## Open

- Commit signing: ed25519 key generated, git signs locally (`%G?` = `G`);
  upload needs Eric to run `gh auth refresh -h github.com -s admin:ssh_signing_key`.
  Then: add `required_signatures` to the master ruleset.
- Donations: GitHub Sponsors needs Eric's own enrolment; add `.github/FUNDING.yml` after.
- CodeQL default setup will keep flagging new test fixtures; an advanced
  setup with `paths-ignore` for tests would stop it.
