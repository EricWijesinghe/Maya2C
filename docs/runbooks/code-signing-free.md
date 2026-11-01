# Runbook: free Windows code signing (SignPath Foundation), Eric applies

Status: **no record that an application was sent** (searched STATE, DECISIONS, reports, PRs and
memory on 2026-11-01). Eric approved code signing on 2026-11-01: try this free route first, and pay
(about $10/month, Azure Trusted Signing) only if it is refused.

## Why this route
Certificate authorities do not issue code-signing certificates to projects without a legal entity,
and they charge. SignPath Foundation issues one free to open-source projects, with the Foundation
as publisher. Maya2C meets the stated basics:
- a public GitHub repository;
- an OSI licence: Apache-2.0 (`LICENSE`, and `license` in `Cargo.toml`);
- binaries built only by CI (`.github/workflows/release-binaries.yml`);
- a published policy page: https://maya2c.dev/guides/code-signing-policy/
  (source `docs/site/authored/code-signing-policy.md`).

Sources, read 2026-11-01: https://signpath.org/terms, https://signpath.org/about,
https://about.signpath.io/product/open-source. Quotas and the full checklist were not in what we
read, so confirm them on the form.

## Eric, about 5 minutes
1. GitHub: two-factor authentication on (Settings > Password and authentication). SignPath requires
   it for every team member.
2. Apply at https://signpath.org/ with:
   - Project: Maya2C, https://github.com/EricWijesinghe/Maya2C, Apache-2.0.
   - Signed artifacts: the Windows release binaries from `release-binaries.yml` on `node-v*` tags
     (node and CLI, Maya Wallet, Maya Chat, Maya2C Hub). Nothing built on a personal machine.
   - Build: GitHub Actions with public logs. The `sign` job runs in the `release` environment and
     needs the owner's approval.
   - Policy page: https://maya2c.dev/guides/code-signing-policy/
   - Team: Eric Wijesinghe (owner, committer, approver).
3. Tell Claude the outcome.

## After approval (Claude)
- In `release-binaries.yml`, replace `scripts/authenticode-placeholder.sh` with the SignPath GitHub
  action, in the `sign` job only, since it already holds the only OIDC token.
- Add the credit SignPath requires to the policy page and the release notes: "Free code signing
  provided by SignPath.io, certificate by SignPath Foundation".
- Update the policy page's status row.

## If refused
- Windows: Azure Trusted Signing, about $10/month, with identity validation. Eric pays (his
  2026-11-01 note).
- Linux: GPG signatures are free and ours to make.
- macOS: Apple Developer, $99/year, after the MacBook arrives.
