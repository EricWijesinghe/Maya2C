# Architectural Directives
- Engine: Rust 2021 Edition, Tokio async runtime, RocksDB storage.
- Constraints: Never use `unsafe` without comments. Do not commit failing tests.
- Safety: Handle all `Result` types explicitly; no unwrap() in production code paths.
- Execution: Always run `cargo clippy` and `cargo test` to verify changes — via the filtered wrappers `ql` and `qt` (see below), never raw.

# Token Optimization & Output Directives

- Response Style: Terse, high-density, fragment-based ("Caveman Mode"). Omit intros, conversational setup, and explanation of what you are about to do. Front-load `file:line` references.
- Code Navigation: ALWAYS use `ast-grep` for structural queries and `rg` (ripgrep) for pattern search. Never read a raw file in full unless it is the explicit target of the task. Invoke as `ast-grep`, not the `sg` shim — `sg` prints a deprecation banner on every call.
- Command Execution: Never run unfiltered test, build, or status commands. Pipe through `condense`, `head`, or `rg`. Use `qt` / `qb` / `ql` instead of bare `cargo test` / `cargo build` / `cargo clippy`.
- File Reading: Use precise offset/limit range reads or `@path/to/file` mentions. No speculative whole-file reads.
- Context Memory: When context grows large, tell the user to run `/compact`, or write `.claude/session-handoff.md` before `/clear`.

## Filtered Command Reference

| Instead of | Use | Effect |
|---|---|---|
| `cargo test` | `qt` | failures + 5 lines context, capped at 40 |
| `cargo build` | `qb` | errors only |
| `cargo clippy` | `ql` | warnings/errors only |
| `git status` | `gst` | short + branch, capped at 30 |
| `git log` | `glog` | last 5, one line each |
| `git diff` | `gdf` | `--stat` summary, never full hunks |
| grep/read loop | `sgr '<pattern>'` | ast-grep structural match, capped at 60 |
| reading many files | `pack` | repomix tree-sitter compressed pack |
| sizing a codebase | `stats` | scc language/line summary |
| any noisy command | `<cmd> 2>&1 \| condense` | strips ANSI/progress, dedupes, keeps diagnostics |