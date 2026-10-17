# 2026-10-18 — Overnight run: sealed keys, proofs, sites, docs, wallet

Eric, 02:32 KST: "do your max until morning 9 AM", with the build list (security first), then 3D websites, social footer, the GitHub page and layered docs. Later additions:

- every page alive with 3D while scrolling, and very colourful;
- a different look per surface;
- mobile responsive;
- portfolio cross-links and support links;
- docs readable by a child, an engineer and a scientist;
- "kill switches and full control".

Every number below comes from a command run in this session.

## Security found and fixed on the live machine (2026-10-17 audit)

- **Validator keys were plaintext and readable by every account.** This covered the four ML-DSA-65 `validator.key` files (seed, v2–v4), `secrets\`, and `data\node_key`. They are now restricted to EricW and SYSTEM, with inheritance cut.
  - The original ACLs are backed up in `D:\Temp\claude\acl-backup-20261017\`.
  - **Incident:** the first `icacls` attempt ran `/inheritance:r` before `/grant` and left the files with an *empty* ACL. It was restored from the backup within minutes. All four validators kept committing throughout (`get_bft_status` committee 4).
- **BitLocker is off on C: and D:.** This is left for Eric, who must hold the recovery key.
- **The old branch checkout used rustls 0.23.45 and wasmtime 48.0.4** (15 advisories, including a PQ→classical downgrade). master already had 0.23.46 and 48.0.6, and `cargo deny check` on master reported `advisories ok, bans ok, licenses ok, sources ok`. The checkout was moved to master.
- **Windows Defender's ClickFix/Commando detections** are from 2026-10-06 (the CDN incident's own investigation command) and were already handled. No new detections.

## Merged

| PR | What |
|---|---|
| #123 | Windows install CI progress |
| #119 | Live network dashboard + `get_peers` (no addresses exposed: checked) |
| #116 | ADR-047 single-PC foundation set (gate 5), conflicts resolved |
| #128 | README + GitHub presentation, Support section, animated SVGs |
| #129 | maya2c.dev: living 3D background on every page, per-section palettes, mobile fixes (0 overflow offenders on 75 pages at 360–768 px), portfolio + socials footer, Support page |

## Opened, waiting on CI or on Eric

| PR | What | State |
|---|---|---|
| #131 | **Sealed validator keys (ADR-049)** | CI |
| #130 | Release SBOM (cargo-cyclonedx) + signed SLSA provenance | CI; release path NOT VERIFIED (needs a tag) |
| #134 | **Kani proof of the 256-bit long-division step** | CI |
| #133 | Layered docs: plain words / engineers / scientists, KaTeX, 4 interactive math graphics | CI |
| #132 | ADR-050 owner control plane + ADR-051 level-5 PQ (both Proposed) | Eric decides |
| #135 | Wallet GUI 256-bit amounts | Needs Eric to look at the send screen |

### #131 sealed keys

The format is Argon2id + ChaCha20-Poly1305, with every cleartext field bound as AAD. A production build refuses plaintext keys. A devnet build warns. The wallet opens both formats.

Real output:

- `cargo test -p maya-signer -p maya2c-node -p l1-wallet`: 66 passed, 0 failed.
- Production feature (`--features production`, `validator_key` tests): 9 passed, 0 failed.
- clippy: exit 0, no warnings in changed files.
- `cargo xtask coverage --verify-targets`: "all claims backed".
- `claims-check`: 0 unauthorised.

The CLI rehearsal with the real binary:

- passphrase flag after the command → sealed;
- plaintext key → warning;
- `--seal-validator-key` keeps the public key (`451d950a…` before and after);
- refuses to overwrite;
- refuses an already-sealed input.

AI rust-reviewer and security-reviewer passes found no critical issues. The HIGH and MEDIUM findings are fixed in `89a15ecf`:

- flag order;
- plaintext generation under production;
- atomic writes;
- KDF bounds;
- newline rules.

CodeQL flagged 12 `rust/cleartext-logging` alerts. All were false positives: the taint was the passphrase-file *path* error. They are dismissed with that reason.

### #134 Kani

`long_division_step_keeps_the_invariant` proves one restoring-division step for every 256-bit divisor wider than 64 bits and every remainder below it.

- `VERIFICATION:- SUCCESSFUL`, 3.29 s.
- Seeded mutations:
  - borrow dropped in `sub` → FAILED;
  - wrong carry in `shl1_or` → FAILED.
- Also SUCCESSFUL: `bytes_round_trip`, `division_by_zero_is_refused`.
- `mul_then_div_round_trips` (pre-existing) did not finish within 40 minutes: **NOT VERIFIED**. It is being rerun.

### #135 wallet GUI

Every balance, amount and fee is `U256`. IPC amounts are strings, the parser is strict and length-capped, display has 18 decimals, and no float is used anywhere.

- core tests: 30 + 5 + 21 + 31 + 17 passed, 0 failed.
- src-tauri: 2 passed.
- UI: `wasm32` check clean.
- A security review found one MEDIUM: the "units" label meant base units, but values are now whole coins. Fixed: the label now says "coins".
- **The coin has no official ticker** (`chain/maya2c-testnet.json` `symbol: null`; `mesh-api` uses `MAYA`). **Eric decides.**

## Deployed

- **eric.maya2c.dev:** rebuilt as a versioned Astro + three.js site. The source is private repo `EricWijesinghe/eric-site`.
  - Netlify deploy `6ad3ba650f8cafcb3cefccda`. The previous deploy is `6ad107c813917d585a5682b0`.
  - Live check (curl): new title; strict CSP `script-src 'self'`; HSTS, X-Frame-Options DENY; 0 external scripts; LinkedIn, Facebook and Instagram links.
  - Lighthouse mobile 100/100/100/100. Desktop performance 70 with WebGL under software GL; real GPU NOT VERIFIED.
  - Phone QA at 360/390/414/768 px: no horizontal overflow, tap targets ≥ 44 px.
- **GitHub profile:** the new public repo `EricWijesinghe/EricWijesinghe` has a README in its own aurora/neon-circuit look.
- **maya2c.dev:** deploys from master through `site.yml` after #129 (run queued at 04:27).

## Not done, and why

- **Live testnet key migration (sealing the four keys).** It needs a restart of each validator, one at a time, with Eric present, and the 7-day run is in progress.
- **Funding links.** There is no GitHub Sponsors listing (`hasSponsorsListing=false`) and no Buy Me a Coffee account. Inventing links would be dishonest. The sites carry one config constant each for a URL.
- **"Full control" beyond ADR-050.** A master key over other people's funds is a backdoor. ADR-050 proposes the safe version: Eric's operator control, plus a 2-of-3 bounded council pause the code already supports. The testnet genesis has no council today.
- **The `glib` 0.18 Dependabot alert** (apps/hub, GHSA-wrw7-89jp-8q8g). It is pinned by Tauri's GTK3 stack, there is no compatible fix, and the iterator is not called.
- **Code-signing certificate.** It costs money: Eric decides. SignPath Foundation (free for OSS) is the first option to try.

## Web and DNS check on maya2c.dev (read-only, 04:40)

`curl -sI https://maya2c.dev/` returns only `strict-transport-security: max-age=31556952` (behind Cloudflare). The page carries a `<meta>` CSP (`default-src 'self'`). Missing at the edge, or not set:

| Item | Now | Fix (Cloudflare dashboard; Eric, since Claude holds no Cloudflare token after the 10-06 incident) |
|---|---|---|
| `X-Frame-Options` / `frame-ancestors` | absent (a meta CSP cannot set `frame-ancestors`) | Rules → Transform Rules → Response headers: `X-Frame-Options: DENY` |
| `X-Content-Type-Options`, `Referrer-Policy`, `Permissions-Policy` | absent | same rule: `nosniff`, `strict-origin-when-cross-origin`, `camera=(), microphone=(), geolocation=()` |
| DNSSEC | off (no DS record) | DNS → Settings → Enable DNSSEC, then add the DS at the registrar |
| CAA | none | DNS: `CAA 0 issue "letsencrypt.org"`, `CAA 0 issue "pki.goog"`, `CAA 0 iodef "mailto:<your email>"` |
| SPF / DMARC | none, so anyone can spoof @maya2c.dev mail | If the domain sends no mail: `TXT "v=spf1 -all"`, `_dmarc TXT "v=DMARC1; p=reject"`, null MX `MX 0 .` |
