---
title: Contributors & badges
description: What earns a Maya2C contributor badge, how to claim one, and where the badges are listed. Badges carry no token and no money.
---

Badges say what someone has done for Maya2C. They are a record, not a reward:
they carry no token, no money, and no promise of either.

## The badges

| Badge | Criteria |
|---|---|
| ![Testnet node, 7 days](/badges/testnet-7-day.svg) | Ran a validator or full node on `maya-testnet-1` for at least 7 consecutive days. |
| ![Security finder](/badges/security-finder.svg) | Reported a valid vulnerability privately under [SECURITY.md](https://github.com/EricWijesinghe/Maya2C/blob/master/SECURITY.md), with consent to be named. |
| ![Merged PR](/badges/merged-pr.svg) | Had at least one pull request merged into `master`. |

A person can hold more than one badge.

## How to claim

Open an issue on the
[Maya2C repository](https://github.com/EricWijesinghe/Maya2C/issues/new) and
title it `Badge claim: <badge name>`. Include:

- **Testnet node:** your node's peer ID, the dates it ran, and a log excerpt
  with keys and addresses removed.
- **Security finder:** the advisory link, or confirmation that the report was
  made privately. Do not put vulnerability details in the public issue.
- **Merged PR:** the pull request number.

The maintainer checks the claim against the criteria and adds an entry to
[`contributors.json`](/badges/contributors.json). Claims that cannot be checked
are declined with the reason given.

## Where they are listed

- The machine-readable list is
  [`contributors.json`](/badges/contributors.json). It starts empty.
- Security reporters are also named on the
  [Security Hall of Fame](/guides/security-hall-of-fame/).

No one is listed until a claim has been checked. Nothing here is taken from a
scanner's output or from a name without a claim.
