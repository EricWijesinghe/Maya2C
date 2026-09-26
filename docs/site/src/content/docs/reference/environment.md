---
title: 'Development environment'
editUrl: false
# GENERATED from docs/environment.md by scripts/ingest.mjs. Edit the source, not this.
---
How this repository is wired into the machine and the agent harness around it:
which MCP servers are enabled and why the rest are not, which ECC skills and
agents are installed, what `.ignore` excludes, how brand assets are deployed,
and where the dependency caches live.

None of this is about the chain. It is about the cost of working on it — a
tool nobody calls is a tax on every session, and a cache in the wrong place
fills the wrong volume.

Moved out of `CLAUDE.md` on 2026-09-20, verbatim.

## MCP Servers

Project scope, `.mcp.json`, enabled in `.claude/settings.local.json`. Tool
search defers full schemas, but every server still puts its tool *names* and its
instructions block in the prompt on every turn. So a server nobody calls, or a
tool nobody calls, is a permanent tax. The set below was cut to what 18 sessions
of transcripts show being used (2026-09-11), and each server has one job so that
two of them are never competing for the same question:

| Question | Server and tool |
|---|---|
| Where is `X` defined? What is in this file? | `serena` `find_symbol` / `get_symbols_overview` (rust-analyzer: exact) |
| Who uses `X`? Rename `X` everywhere. | `serena` `find_referencing_symbols` / `rename_symbol` |
| What calls what, across crates? What is the shape of this subsystem? | `codebase-memory-mcp` `trace_path` / `get_architecture` / `search_graph` |
| A crate's API, before depending on it or calling a part of it this repo does not use yet | `context7` `resolve-library-id` then `query-docs` (the `docs-lookup` agent runs on it) |
| One known URL (an RFC, a FIPS spec, docs.rs) | `fetch` |

- **`serena`**: its file-read, shell, memory and text-insertion tools are
  excluded in `.serena/project.yml`, because the built-ins already do those jobs.
- **`codebase-memory-mcp`** (0.10.8): `.mcp.json` overrides the user-scope entry
  with `--tool-profile=scout`, which cuts it from 15 tools to 7 and from 24.9K to
  13.8K schema characters.
  - Re-indexing is not in the scout profile. Re-index with
    `codebase-memory-mcp cli index_repository '{"repo_path":"D:/Maya2C","mode":"full"}'`.
  - `claude mcp list` warns that the server is defined in two scopes. That is the
    override working, not a fault.
  - Version 0.9.0 silently skipped files.

Disabled deliberately:
- **Project servers, via `disabledMcpjsonServers`:**
  - `filesystem` duplicates Read/Write/Edit/Glob.
  - `git` duplicates Bash git, which is allowlisted in `.claude/settings.json`.
  - `memory` duplicates `codebase-memory-mcp` and the file memory under
    `~/.claude/projects/`.
  - `sequential-thinking` duplicates built-in extended thinking.
  - `headroom` was never called. Its `headroom_compress` takes the text as an
    argument, so that text is already in context before anything is compressed.
    It cannot save tokens from inside the conversation.
- **User servers and connectors, via `disabledMcpServers`:**
  - `rustrover` was never called, and adds about 40 tool names (SQL, database,
    run-configuration tools) that serena and the shell already cover. It is
    also only live while the IDE runs.
  - The claude.ai connectors `Shopify` and `Viewmax` add about 80 tool names
    between them and have nothing to do with a blockchain.

All of these are reversible: remove the name from the list. The pre-trim configs
are in `~/.claude/backups/mcp-trim-20260911/`.

**Research tooling is shell-side, not MCP**, so it costs nothing until it's used:

| Tool | Use it for |
|---|---|
| `agent-reach` skill (`~/.claude/skills/agent-reach/`) | Multi-source research. It routes to the tools below. The CLI is pinned to upstream `Panniantong/Agent-Reach@da5044d`; do not run `check-update` |
| `mcporter call 'exa.web_search_exa(query: "...", numResults: 5)'` | Semantic web search: papers, advisories, standards. Exa is configured in `~/.mcporter/mcporter.json` |
| `yt-dlp --write-auto-sub --skip-download` | Conference-talk transcripts. `~/.config/yt-dlp/config` sets `--js-runtimes node` |
| `gh` | Issues, PRs, releases, `gh search code`. **Needs `gh auth login` once** |

Social channels (X, Reddit, …) are deliberately unconfigured: they need the
user's browser cookies. Agent-Reach's own MCP server exposes only `get_status`,
so it is not registered.

**Graphify is a skill, not an MCP server** — invoke with `/graphify`.

## Harness Surface (ECC + plugins)

`~/.claude` runs ECC 2.2.1 as a **manual install**, not the `ecc@ecc` plugin —
never run `/plugin install ecc@ecc` on top of it, that duplicates every skill,
command and hook. The same tax that applies to MCP tool names applies here:
every skill, agent and command puts its name and description in the prompt on
every turn. The full `developer` profile measured **~18.1K tokens/turn**, so
the surface is curated to this stack and the rest is archived, not deleted, in
`~/.claude/backups/curated-20260912/`:

| Surface | Installed | Archived | Cost |
|---|---:|---:|---:|
| Skills | 43 | 85 | ~3.7K |
| Agents | 27 | 41 | ~1.8K |
| Commands | 45 | 49 | ~1.2K |
| Plugins (4) | — | — | ~1.3K |

Kept language packs are the ones this repo actually contains: `rust-*`,
`cpp-*` (cuda-miner), `python-*` (scripts/), `typescript-reviewer` (sdk-js,
docs/site). Everything Django/Laravel/Vue/Flutter/Kotlin/Swift/SEO/marketing is
archived. Restore one by moving the file back — do not re-run
`ecc install --profile developer` to get it, that restores all 240 files and
the full 24-entry hook graph.

Plugins (user scope, official marketplace only): `rust-analyzer-lsp`,
`claude-security` (8 scanning agents — the reason to keep the priciest plugin
on a consensus/crypto codebase), `skill-creator`, `claude-md-management`,
`claude-code-setup`. Deliberately **not** installed: `github` and `serena`
plugins (the `gh` CLI and the serena MCP server already cover them), `semgrep`
and `sonarqube` (thin Rust rule coverage next to clippy + `cargo audit` +
`cargo deny`, and a permanent tool-name cost), `pr-review-toolkit` and
`code-review` (ECC ships the same agents), `superpowers` (a third overlapping
TDD/verification/review system makes agent selection worse), `codspeed` (wants
a CI account), `security-guidance` (POSIX shell hooks plus an LLM diff review
on every Stop).

**Project agents** (`.claude/agents/`, ~0.6K tokens/turn): nine curated from
`msitarzewski/agency-agents@ad9264e` (MIT) out of 295 —
`blockchain-security-auditor`, `security-architect`, `codebase-archaeologist`,
`research-synthesist` (read-only `tools:`), plus `rust-refactoring-specialist`,
`webassembly-engineer`, `minimal-change-engineer`, `sre`,
`desktop-app-engineer`. Each has its upstream path in a frontmatter comment and
a trailing *Maya2C Operating Context* section that overrides the generic
web/EVM body. Skipped on purpose: upstream `code-reviewer` and
`software-architect` (collide with ECC's), `reality-checker` (Laravel +
Playwright screenshots), `solidity-*` (no EVM here), and every non-engineering
division. Add one by hand in the same shape — do not run upstream
`install.sh --tool claude-code`, which copies all 295 into `~/.claude/agents/`.

Nine ECC hooks are wired in `~/.claude/settings.json`; 15 were removed and are
listed in `env.ECC_DISABLED_HOOKS`. `pre:bash:dispatcher` in particular blocks
the first Bash call of every session with a GateGuard prompt. Re-running any
ECC install or repair restores all of them — re-trim afterwards.

## Ignore Rules

`.ignore` at the repo root is the real exclusion file: the Claude Code binary
references `.ignore`, `.rgignore`, and `.gitignore`, and contains **no**
reference to `.claudeignore` — the `.claudeignore` that used to live here was
inert and has been removed. `.gitignore` cannot exclude *tracked* paths, which is
why `.ignore` carries `apps/wallet-gui/ui/target` and `target-contracts`.
`.claude/settings.json` adds `permissions.deny` as a hard backstop.

## Branding

`logo-assets/` is the source of truth and is never edited in place. Three
commands deploy it; the two scripts take `--check` so drift is a failure rather
than a discovery:

```powershell
python scripts/deploy_brand_assets.py    # favicons + wordmarks into each app
python scripts/make_og_card.py           # the 1200x630 social card
cargo tauri icon logo-assets/print/HighRes-Square-2000_2000x2000.png `
  -o apps/wallet-gui/src-tauri/icons          # the desktop icon set
```

Two pack defects are corrected on copy, not propagated: `site.webmanifest` ships
an empty `name`/`short_name`, and `paste-in-head.html` hardcodes root-absolute
paths that only suit the explorer.

Reference checking differs per surface: `apps/dashboard/` and `apps/wallet-gui/ui/` fail
`trunk build` on a missing `data-trunk` dir, then `scripts/check_brand_refs.py`
walks each built `dist/`; `apps/explorer/` has no build step, so
`crates/node/tests/server_tests.rs` asks the running server for every path its rendered HTML
names. The explorer resolves `--assets` against its working directory, logs that
directory at startup, and warns loudly when it is absent — a `ServeDir` over a
missing path would 404 every icon and look like a browser problem.

## Caches

Dependency caches live on **D:** (`UV_CACHE_DIR=D:\Caches\uv_cache`,
`NPM_CONFIG_CACHE=D:\Caches\npm_cache`, set at user scope and pinned again in
`.mcp.json`). Do not create parallel cache dirs.

Nothing this project generates should land on **C:**, which ran down to
26.7 GB free on 2026-09-22. Moved off it that day:

| What | Now | Why it is safe |
|---|---|---|
| `CARGO_HOME` (registry, `bin/`, advisory-db) | `D:\Caches\cargo` | moved whole, not re-downloaded; `C:\Users\EricW\.cargo` is a junction to it, so `PATH` is unchanged |
| `RUSTUP_HOME` (toolchains) | `D:\Caches\rustup` | same, junction at `C:\Users\EricW\.rustup` |
| WSL `Ubuntu-24.04` disk (55.6 GB) | `E:\WSL\Ubuntu-24.04\ext4.vhdx` | `wsl --manage Ubuntu-24.04 --move`, the supported path |
| `TEMP` / `TMP` (user scope) | `D:\Temp` | rust-analyzer copies every proc-macro DLL into `TEMP` per session (`proc-macro-srv*`, up to 887 MB each, 36 were left behind) |

`CARGO_HOME` and `RUSTUP_HOME` are also set at user scope. The junctions stay
anyway: anything that resolved `~/.cargo` before the move still finds it.
