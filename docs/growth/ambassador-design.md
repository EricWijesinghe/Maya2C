# Growth design: ambassadors, referrals, retention, funnels

Status: **design only, nothing is open.** Lane H of
`reports/audits/2026-11-12-brand-ux-devex-seo.md` (branch
`docs/audit-2026-11-12-brand-ux`), sections 3 and 5. Nothing in this file is
live until the maintainer opens it; the issue form in §1.3 is a draft, not a
file in `.github/`.

The rule that shapes every section: **nothing is paid, in any asset or in
money, and nobody talks about price or returns.** Recognition is the existing
badge system on [`/guides/contributors/`](../site/authored/contributors.md)
plus named credit. §5 says why, and what would have to happen before that
could change.

## 0. What exists today (checked 2026-11-13)

| Piece | Where | State |
|---|---|---|
| Contributor badges (testnet node 7 days, security finder, merged PR) | `docs/site/authored/contributors.md`, `docs/site/public/badges/` | live; `contributors.json` starts empty |
| Security Hall of Fame | `docs/site/authored/security-hall-of-fame.md` | live |
| Code of conduct | `CODE_OF_CONDUCT.md` | live, 15 lines |
| Contribution model | `CONTRIBUTING.md` | suggestions; only the maintainer merges; most accepted suggestions are re-implemented, not merged as written |
| Issue forms | `.github/ISSUE_TEMPLATE/` (bug-report, suggestion, validator-application) | live; the validator form already says "recognition only" |
| Labels | `good first issue`, `help wanted` | exist on GitHub |
| Release notes | `docs/releases/node-v0.1.0-testnet.*.md` | five files |
| One-line node install ("the kit") | `docs/site/authored/help-run.mdx` | live |
| Ecosystem metrics | `cargo xtask eco-metrics`, `docs/ecosystem/METRICS.md` | counts from git only, no emails printed |
| Site analytics | none in `docs/site`; `code-signing-policy.md:56` states the apps send no analytics | **MISSING** |

## 1. Ambassador programme

### 1.1 Roles

All roles are volunteer and unpaid. A person can hold more than one.

| Role | Does | Does not |
|---|---|---|
| **Explainer** | Writes or talks about Maya2C (posts, talks, videos, translations) using only the claims in the audit's §4 table. | Mention price, returns, listings, "investment", or anything about a token sale. |
| **Guide** | Answers newcomer questions in Discussions; points to the quickstart, `help-run`, and the right issue form. | Give support in private channels that ask for keys, logs with keys, or money; promise fixes or dates. |
| **Node helper** | Helps people run the one-line install from `help-run`, reports the friction they hit as issues. | Run nodes for other people, hold anyone's keys, or operate infrastructure in the project's name. |
| **Translator** | Translates guides; a maintainer-merged translation is credited by name. | Translate claims that are not in English source. |

An ambassador speaks for themself. Only the maintainer speaks for the project.
An ambassador has no write access, no moderation rights, and no say over
merges (`CONTRIBUTING.md` already says no access is handed out).

### 1.2 Code of conduct for ambassadors

Everything in `CODE_OF_CONDUCT.md`, plus:

1. **Disclose the role every time.** Every post, talk, video or reply about
   Maya2C names the affiliation in the post itself, not only in a profile:
   "I am a volunteer Maya2C ambassador (unpaid)." A link-in-bio is not enough.
2. **No price, no returns.** No statement about what a coin is or will be
   worth, no earnings, yield, payback or "get in early". Mining and staking
   are described as the guides describe them; `start-mining.mdx` already says
   no profit is promised or estimated.
3. **Claims from the list only.** Use the wording in the audit's §4
   "Verifiable claims" table, with its evidence link. A claim whose evidence
   is MISSING is not made. No "first", "only" or ranking statements
   (`cargo xtask claims-check` enforces this on the repo; ambassadors follow
   the same rule off it).
4. **No paid promotion of Maya2C, by anyone.** An ambassador who is paid by
   any third party to talk about Maya2C cannot be an ambassador.
5. **Never ask for keys, seed phrases, passwords or money**, and tell anyone
   who is asked by someone claiming to be from Maya2C that it is a scam.
6. **No impersonation.** No "official" handles, no use of the emblem as an
   avatar, no domain or channel that looks like the project's (`BRAND.md`).
7. **Private data stays private.** Never post another person's logs, peer
   IDs, addresses or contact details without their consent.

### 1.3 Application: GitHub issue form (draft)

To ship, copy to `.github/ISSUE_TEMPLATE/ambassador-application.yml`. It
follows `validator-application.yml`.

```yaml
name: Ambassador application
description: Volunteer to explain Maya2C or help newcomers. Unpaid; recognition only.
title: "Ambassador application: <your handle>"
labels: ["ambassador-application"]
body:
  - type: markdown
    attributes:
      value: |
        Ambassadors are **volunteers**. Nothing is paid, in any token or in
        money, now or later, and no allocation is promised. Recognition is a
        listing on https://maya2c.dev/guides/contributors/.

        Read the ambassador code first:
        https://github.com/EricWijesinghe/Maya2C/blob/master/docs/growth/ambassador-design.md#12-code-of-conduct-for-ambassadors

        Do not put an email, phone number or real name here unless you want it public.
  - type: dropdown
    id: role
    attributes:
      label: Role
      multiple: true
      options: [Explainer, Guide, Node helper, Translator]
    validations:
      required: true
  - type: textarea
    id: work
    attributes:
      label: Something you have already done
      description: A link to a post, answer, issue, PR or translation about Maya2C or similar open-source work.
    validations:
      required: true
  - type: input
    id: referred_by
    attributes:
      label: Who pointed you here? (optional)
      description: A GitHub handle, only if that person agreed to be named. Leave blank otherwise.
  - type: checkboxes
    id: terms
    attributes:
      label: Commitments
      options:
        - label: I understand this is unpaid and earns no token, money or allocation.
          required: true
        - label: I will say I am a volunteer Maya2C ambassador in every post about Maya2C.
          required: true
        - label: I will not discuss price, returns or earnings.
          required: true
        - label: I am not paid by anyone to promote Maya2C.
          required: true
```

The label `ambassador-application` does not exist yet; create it when the
form ships.

### 1.4 Review and acceptance

1. The maintainer reads the linked work against §1.2.
2. Accepted: a comment on the issue, and an entry in `contributors.json` with
   `"role": "ambassador"` and the roles chosen. Declined: a comment with the
   reason. No silent declines.
3. A 30-day check: the first three public posts are linked in the issue so
   disclosure can be checked.

### 1.5 Recognition (the whole list)

- A listing under "Ambassadors" on the contributors page, from
  `contributors.json`.
- The existing badges when their own criteria are met (a Guide whose PR is
  merged earns the merged-PR badge the normal way). No badge is granted for
  being an ambassador alone. An "Ambassador" badge SVG would be a later,
  separate change.
- Named credit in release notes for translations, guides and reported
  friction that led to a fix.
- Early access to test builds, which anyone can already download.

Not on the list, ever, under this design: tokens, testnet or mainnet
allocations, airdrops, NFTs, gift cards, merchandise sold or given in
exchange for posting, fees, or "points" redeemable for anything.

### 1.6 Exit and removal

- **Leaving:** comment on the application issue or open any issue saying so.
  The listing is removed within 7 days, or kept as "former ambassador" if the
  person asks.
- **Inactivity:** after 180 days with no linked activity, the maintainer asks
  once; no answer in 30 days moves the person to "former".
- **Removal:** for a breach of §1.2. Rules 2, 4, 5 and 6 (price talk, paid
  promotion, asking for secrets or money, impersonation) mean immediate
  removal. Others get one written warning on the issue first.
- **After removal:** the listing goes; badges earned on their own criteria
  stay, because they record work that happened. If the person kept posting as
  an ambassador, the project posts one dated line saying they are no longer
  one. The reason is stated on the issue without personal details.
- **Appeal:** one reply on the issue; the maintainer's answer is final.

## 2. Referral system (non-monetary)

A referral is **attribution**: "this person pointed me here", recorded so the
referrer can be thanked. Nothing else.

### 2.1 How it works

1. Forms that lead to a recognised outcome (badge claim, ambassador
   application, validator application) carry the optional field
   "Who pointed you here?" from §1.3.
2. Both sides opt in: the referred person fills the field; the referrer is
   asked on the issue whether they want to be named. No answer means no
   credit and the handle is removed from the record.
3. Credit is recorded only when the referred person's **outcome is checked**:
   a badge claim accepted, a PR merged or a suggestion re-implemented, a
   validator listed. Starring, signing up or opening an issue earns nothing.
4. The referrer's line on the contributors page reads "helped N people make a
   checked contribution", with no ranking and no leaderboard.

### 2.2 Privacy

- **No PII collected.** A public GitHub handle is the only identifier, and
  only one the person typed and agreed to show.
- **No referral links, no codes, no URL parameters, no cookies, no tracking
  pixels, no fingerprinting**, on the site, in the apps, or in shared links.
  The apps already send no analytics (`code-signing-policy.md:56`).
- Nothing is stored outside the public issue and `contributors.json`.
  Deleting the field on the issue, or asking, removes the credit.

### 2.3 Abuse resistance

| Abuse | Why it does not pay |
|---|---|
| Sock-puppet "referred" accounts | Credit needs a checked outcome; a fake account has to produce real, reviewed work. |
| Self-referral | The maintainer drops a referral where the handles are the same person; there is nothing to gain from one anyway. |
| Spam recruiting | Nothing is worth farming: no payout, no rank, no level. Spam breaks the CoC and leads to removal under §1.6. |
| Multi-level chains | One level only. A referrer gets no credit for the people their referrals bring in. |
| Pressure on newcomers to name someone | The field is optional and says so; naming nobody changes nothing about how a claim is treated. |
| Gaming a count | The count is shown, not ranked, and is reviewed by hand with the outcome it rests on. |

## 3. Developer retention

### 3.1 Good-first-issue flow

1. The maintainer labels small, self-contained issues `good first issue` with
   four things in the body: the files involved, the check that proves it
   (`cargo test -p <crate> <name>` or a doc build), what "done" looks like,
   and who answers questions.
2. The newcomer comments to claim it. A claim lapses after 14 days without a
   push or a comment, and the issue is relabelled.
3. A PR from a fork waits for the maintainer to approve CI
   (`CONTRIBUTING.md`); the first review comment arrives with that approval.
4. Because most suggestions are re-implemented rather than merged as written,
   an accepted suggestion credits the author in the commit (`Co-authored-by:`)
   and in the release notes. Whether a re-implemented suggestion earns the
   merged-PR badge is the maintainer's call; this design suggests a separate
   "accepted suggestion" line in `contributors.json` rather than changing the
   badge's criteria.

### 3.2 Mentorship rota

There is one maintainer today, so the rota starts with one name and grows only
with people who have a merged-PR badge and volunteer. A rota entry is: handle,
areas (docs, site, wallet, node), and the week they answer `good first issue`
questions. It lives in this file's §3.2 table once there is more than one
name. Mentors are unpaid and follow §1.2.

### 3.3 Office hours

A fixed, public slot announced in Discussions, recorded as a Discussions
thread (questions and answers in text) rather than a call with sign-ups, so
nobody has to share an email or a calendar. Cadence and time are the
maintainer's decision; nothing is announced until one is chosen.

### 3.4 Release notes

Each `docs/releases/*.md` gains a "Thanks" section listing, by public handle
and only with consent: merged or re-implemented contributions, reported bugs
that were fixed, translations, and security reports (as in the Hall of Fame).

### 3.5 "Your PR merged" thank-you flow

On merge (or when a suggestion is re-implemented):

1. A comment on the PR: what shipped, the release it will be in, and the link
   to claim the merged-PR badge (`Badge claim: Merged PR`).
2. The contributor is added to the next release notes' "Thanks" section.
3. A pointer to one more `good first issue` in a nearby area, if one exists.

This is a template comment the maintainer posts, not a bot. A bot would need
a workflow with write permission on PRs, which is a separate decision.

## 4. Funnels and their measurement sources

Every step names where its number comes from. **No conversion rate is stated
here, because none has been measured.** A rate is computed only once two
adjacent steps both have a real source for the same period.

| Step | Measurement source | Status |
|---|---|---|
| 1. Visitor to the site | Site analytics | **MISSING, needs Eric's decision on privacy-friendly analytics.** No analytics config exists in `docs/site`. Options to decide between: none (keep MISSING), a cookieless self-hosted counter, or Cloudflare's aggregate request counts for the zone. |
| 1b. Visitor to the GitHub repo | GitHub traffic API: `GET /repos/{owner}/{repo}/traffic/views`, `/traffic/popular/referrers`, `/traffic/popular/paths` | Available to the repo owner's token. **The API keeps only 14 days**, so a scheduled job must snapshot it or the history is lost. No snapshot job exists. |
| 2. GitHub star | `GET /repos/{owner}/{repo}/stargazers` with `Accept: application/vnd.github.star+json` (gives `starred_at`) | Available; counts only, no handles published. |
| 3. Reads the quickstart | Site analytics on `/guides/quickstart/` | **MISSING**, same decision as step 1. `traffic/popular/paths` covers github.com paths only, not the site. |
| 4. Runs a node via the kit (`help-run` one-line install) | No telemetry by design. Proxies: release asset `download_count` from `GET /repos/{owner}/{repo}/releases`; count of `validator-application` issues; checked `testnet-7-day` badge claims. | Partial. Download counts reset because only the newest release is kept (`CONTRIBUTING.md`). A download is not a running node. No public network is running today, so badge claims cannot grow. |
| 5. First PR | GitHub search: `is:pr author:<login>` first appearance; `cargo xtask eco-metrics` for distinct human authors and retention from git | Available. Counts PRs opened, PRs merged, and suggestions re-implemented separately, since most accepted work is re-implemented. |

What would be published, once sources exist: counts per step per month with
the fetch time and source, the same page for everyone (audit Lane I), no
projection and no target line.

## 5. Legal: why nothing is paid

Not legal advice. Plain reasons a lawyer is needed before any of this pays
anyone:

- **Securities law.** Rewarding people with a token, or anything of value,
  for bringing in others who buy or hold makes the token look like an
  investment sold through paid promoters. Many jurisdictions require paid
  promoters of an investment to disclose the payment, and some treat the
  scheme itself as an unregistered offering. The project has said no token is
  for sale (`home.mdx:95`); paid referrals would cut against that.
- **Consumer and advertising law.** A rewarded ambassador who posts without
  saying so is undisclosed paid endorsement, which advertising regulators in
  many countries treat as deceptive. What a rewarded ambassador says about
  value can be attributed to the project.
- **Pyramid and lottery rules.** Rewards that depend on recruits' own recruits
  (multi-level) add pyramid-scheme exposure. Prize draws for sign-ups can be
  regulated as lotteries.
- **Tax and sanctions.** Paying people in many countries creates reporting
  duties and requires screening, which means collecting the personal data §2
  refuses to collect.

**What would change after mainnet, if Eric decides on a funded programme.**
That is his decision, and this design does not propose one. If he does, the
order would be: a lawyer reviews the specific programme in the jurisdictions
it targets; an ADR records the decision; the disclosure line in §1.2 becomes
"paid by the Maya2C project" with the amount basis; payments are fixed fees
for delivered work (a translation, a guide), never tied to price, holdings or
recruits; any KYC and tax handling gets its own privacy review; and this file,
the contributors page and the issue forms are updated in the same change. The
no-price, no-returns and claims-from-the-list rules stay as they are.

## 6. Reviewer checklist (gate for any growth page, form or post)

From the audit's §5. A change fails if any answer is "no".

- [ ] Nothing is paid or promised, in any asset or money, including "later".
- [ ] No price, market value, return, yield, earnings or payback figure.
- [ ] Every claim is in the audit's §4 table with its evidence; nothing marked
      MISSING is stated.
- [ ] No "first", "only", "unique" or ranking wording (`cargo xtask claims-check`).
- [ ] Ambassador content carries the disclosure line in the post itself.
- [ ] No PII beyond a handle the person chose to show; no tracking pixel,
      referral code, URL parameter or cookie.
- [ ] Referral credit is one level and rests on a checked outcome.
- [ ] Any traction number names its source and fetch time, with no projection.
- [ ] Partner logos have written consent on file, or are absent.
- [ ] Nothing contradicts `home.mdx:95` ("no token is for sale") or
      `start-mining.mdx` (no profit promised).
