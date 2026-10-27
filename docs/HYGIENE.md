# Repository hygiene

How the tree stays crawlable. Rules first, then the cleanup ledger.
Map of what lives where: [REPO-MAP.md](REPO-MAP.md).

## Rules

1. **One home per kind of file.** Source in `crates/ bins/ apps/ hal/ sdks/`;
   design prose in `docs/`; evidence in `reports/`; operations in `infra/` and
   `scripts/`. The repo root holds only: manifests, the status/governance docs
   listed in REPO-MAP, and files that tools read by fixed path.
2. **No new root files** without a line in REPO-MAP.md in the same change.
3. **A new top-level directory** needs an ADR line and a REPO-MAP row.
4. **Plans live in `.claude/plans/`** (not `docs/`: `cargo xtask guides-check` scans every command in `docs/`, and a plan names things that do not exist yet); when finished, the outcome moves to
   `PROGRESS.md`/an ADR and the plan is deleted or marked `Superseded`.
5. **Status is stated once.** `features.toml` for "works", `STATE.md` for
   "now". Other docs link; they do not restate counts.
6. **Machine state is ignored, not deleted by hand.** If a tool writes it
   (build dirs, caches, symlink installers, `*.bak-*`), add it to `.gitignore`
   in the same change that first creates it.
7. **Branches and worktrees:** delete a branch once merged; remove its
   worktree once clean. Quarterly: `git branch --merged master`,
   `git worktree list`, `cargo xtask disk`.
8. **Generated files carry a header** saying which command regenerates them.
9. **Never delete evidence** (`reports/`, `incidents/`, ADRs). Supersede it.

## Cleanup ledger

### Done (2026-10-27)

| Action | Why |
|---|---|
| 28 merged local branches deleted | merged into `master`; `git branch -d` refuses unmerged |
| 7 worktrees removed (staking, cb, docs, hud, ci, u256, site-rebuild) | each saved first: cb's one-line DECISIONS edit applied here; hud's WIP committed on `feat/explorer-hud` (unmerged, kept); ci/u256/site-rebuild commits live on their branches; docs' only changes were generated (`health.json`, `src/content`) |
| 10 attacknet daily reports copied to `reports/attacknet/` | were untracked in the runner worktree |
| `.mcp.json.bak-*`, `deploy.sh`, root `genesis.json` deleted | stale backup; legacy compose launcher (image `custom-l1-node`, no compose file at root, referenced by nothing); a 130 B dev-output stub |
| `skills/` (115 AWS skill symlinks) deleted; `/skills/`, `/genesis.json`, `/offsec-sandbox/target/` ignored | installer output; 1.8 GiB nested build dir |
| monitoring configs → `infra/monitoring/`; `ATTACKNET_LAB_GUIDE.md` → `docs/runbooks/attacknet-lab-guide.md`; `deploy-production.sh` → `scripts/` (now `cd`s to the repo root) | root holds only manifests and status docs; references updated in `scripts/attacknet-lab.ps1`, `LAUNCH.md`, infra comments. `bash -n` passes; the script was **not** run (Standing Order 6) |
| `NextGen_Update.md`, `OPTIMIZATION_PLAN.md` → `.claude/plans/` (first put in `docs/plans/`, which CI rejected: their stale crate names failed `guides-check`); `run-claude*` → `scripts/dev/`; `Prompts/` → `docs/master-prompts/` | one home per kind of file |
| `docs/REPO-MAP.md`, this file; linked from README, CLAUDE.md, docs/README | single crawl entry point |

Historical `reports/` still mention the old paths (`./deploy-production.sh`);
they are evidence and are not rewritten.

### Still open

| Item | Why |
|---|---|
| Worktree `D:/Maya2C-wt/attacknet-daily` (5.5 GiB) | the live runner of `scripts/attacknet-daily.ps1` — remove only if that task is retired |
| Worktree `D:/Maya2C-wt-s27` | belongs to another session |
| Branch `feat/explorer-hud` | holds unmerged HUD work (`apps/explorer/assets/hud.{css,js}`) — open a PR or drop |
| Unmerged branches `fix/ci-red-2026-10-19`, `feat/ceremony-u256-v2`, `feat/site-alive` | commits not on master; decide merge or drop |
| `dist/` (249 MiB, ignored) | keep only the latest release artifacts |
| ~25 sibling directories `D:\Maya2C-*` outside the repo (attacknet-*, testnet, validators, peers, drill, incident, nemotron-quarantine, skills-backup …) | runtime state of the testnet/lab; some are live. Inventory and retire per directory — never in bulk |

### Inventory: sibling directories `D:\Maya2C-*` (read-only, 2026-10-27)

Outside the repo, so git neither tracks nor protects them. Sizes were **not
measured** (MISSING). "Newest" is the newest direct child's modified time.
Nothing here was deleted; retire one directory at a time, after checking that
no process or script uses it.

| Directory | Newest | State | Note |
|---|---|---|---|
| `Maya2C-testnet` | 10-23 | **LIVE** | gateway, bootstrap, faucet, chat relay, explorer, status, cloudflared run from `bin\` |
| `Maya2C-validators` | 10-25 | **LIVE** | four `maya2c-node` processes (`pc1`–`pc4`) |
| `Maya2C-incident` | 10-27 | active | incident evidence |
| `Maya2C-ops`, `-peers`, `-eric-site`, `-drill` | 10-19…10-20 | recent | ops logs, PC peers, personal site, DR drill |
| `Maya2C-wt`, `-wt-s27`, `-wt-site`, `-wt-hygiene` | 10-19…10-27 | worktrees | `-wt/attacknet-daily` is the scheduled runner; `-wt-hygiene` is this change |
| `Maya2C-attacknet`, `-attacknet-bootnode`, `-attacknet-v1`…`-v4` | 10-09…10-10 | dormant | lab state; the controller's default paths point here (`bins/attacknet-controller/src/config.rs`) |
| `Maya2C-attacknet-adversary`, `-control`, `-gateway`, `-genesis` | 10-09 | dormant | one child each |
| `Maya2C-rehearsal-20261010`, `-targets`, `-updates` | 10-10…10-13 | dormant | one-off rehearsals |
| `Maya2C-nemotron-quarantine` | 10-10 | dormant | broken lab scripts quarantined on purpose (see memory); keep until Eric decides |
| `Maya2C-launch`, `-profile` | 09-29, 10-18 | dormant | |
| `Maya2C-skills-backup` | 10-18 | junk candidate | backup taken before the AWS skills symlinks were removed from the repo |
