# Workspace hygiene

Disk is the standing hazard on this machine: the volume has been filled to zero
bytes more than once, and a full disk shows up as `os error 112` in the middle
of a test run (2026-10-25: 1.5 GB free, three staking tests failed on it).
`cargo xtask disk` reports artifact size against a 30 GiB ceiling.

## What grows, and what is safe to delete

| What | Grows because | Safe to delete? |
|---|---|---|
| `target/debug/incremental` | every edit/build cycle | Yes. Rebuilt on demand (86 GB on 2026-10-25). |
| `target/llvm-cov-target`, `target/doc`, `target/bench-tps`, `target/mining-localnet` | one-off tool runs | Yes. |
| `target/debug/deps` | every dependency version ever built | Yes, at the cost of a cold build (about 5 min). |
| a worktree's own `target/` | each `git worktree` builds on its own | Yes, with the worktree. |
| `D:\Temp` entries older than two days | test temp dirs, tool scratch | Yes. Leave the current session's scratchpad. |

## Worktrees

One worktree per session (CLAUDE.md). Remove one when its branch is on origin:

```sh
git worktree list
git branch -r --contains "$(git -C <worktree> rev-parse HEAD)"   # must print a branch
git worktree remove <worktree>                                   # refuses if dirty
```

Never `--force` a worktree holding changes you have not read. A worktree whose
only change is `docs/site/public/health.json` is safe to force-remove.

## Branches

```sh
git branch --merged origin/master | rg -v "master"   # then: git branch -d <name>
```

`-d` refuses an unmerged branch, which is the point. Remote branches are
deleted by GitHub when a PR merges.

## Tool folders in the repo root

The `skills` installer drops a `skills/` folder of junctions into one dot-folder
per agent tool. They hold no files of their own (the content is in `.agents/`),
and are ignored by git. Delete a junction with `[IO.Directory]::Delete(path)`
or `cmd /c rmdir`; `rm -rf` on a junction can follow it into the target.
