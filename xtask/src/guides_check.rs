//! `cargo xtask guides-check` — every command a guide tells a developer to
//! run must name something that exists (Master Prompt 17: "every guide's
//! commands run in CI").
//!
//! Running every command is not possible here: some start nodes for minutes,
//! some need Docker, a GPU or another OS. What a stale guide actually does to
//! a newcomer is name a package, test, binary, xtask command, CLI command or
//! script that no longer exists, and that this checks exhaustively:
//!
//! - `cargo <cmd> -p <package>` names a workspace package, and `--test`,
//!   `--bin`, `--example`, `--bench` name one of its targets
//!   (`cargo metadata`);
//! - `cargo xtask <cmd>` is a command this crate dispatches;
//! - `maya2c <cmd>` is a `maya2c` subcommand;
//! - `bash|sh|python|./` `scripts/…` names a file that exists.
//!
//! Anything else is counted as unchecked, not passed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// `cargo xtask` commands, read from `main.rs`'s dispatch arms
/// (`"name" | "alias" => …`) so a new command never needs a second list here.
fn xtask_commands() -> impl Iterator<Item = &'static str> {
    include_str!("main.rs")
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix('"')?.split_once("=>"))
        .flat_map(|(arms, _)| arms.split('|'))
        .map(|arm| arm.trim().trim_matches('"'))
}
/// `maya2c` subcommands (`bins/maya2c-cli/src/main.rs`).
const CLI_COMMANDS: &[&str] = &["dev", "debug", "replay", "fork", "vault", "help"];
const SHELL_LANGS: &[&str] = &[
    "sh",
    "bash",
    "console",
    "shell",
    "powershell",
    "ps1",
    "pwsh",
    "zsh",
];

/// Targets per package: kind ("test", "bin", …) -> names.
type Targets = BTreeMap<String, BTreeMap<String, BTreeSet<String>>>;

fn packages(root: &Path) -> Result<Targets, String> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    let meta: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut all = Targets::new();
    for p in meta["packages"]
        .as_array()
        .ok_or("cargo metadata: no packages")?
    {
        let name = p["name"].as_str().unwrap_or_default().to_string();
        let entry = all.entry(name).or_default();
        for t in p["targets"].as_array().into_iter().flatten() {
            for kind in t["kind"].as_array().into_iter().flatten() {
                let kind = kind.as_str().unwrap_or_default().to_string();
                entry
                    .entry(kind)
                    .or_default()
                    .insert(t["name"].as_str().unwrap_or_default().to_string());
            }
        }
    }
    Ok(all)
}

/// Every `.md` under `dir` into `out`; a directory that cannot be listed goes
/// into `failures`, since its guides would otherwise pass unread.
fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>, failures: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            failures.push(format!("{}: cannot list: {e}", dir.display()));
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            // Generated site copies and vendored trees duplicate `docs/`.
            if !matches!(
                name.as_str(),
                "node_modules" | "target" | "src" | "dist" | ".astro"
            ) {
                markdown_files(&path, out, failures);
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md"))
        {
            out.push(path);
        }
    }
}

/// `(line number, command)` for every command line in a shell code block.
fn commands(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_shell = None::<bool>;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            in_shell = match in_shell {
                Some(_) => None,
                None => Some(SHELL_LANGS.contains(&info.split_whitespace().next().unwrap_or(""))),
            };
            continue;
        }
        if in_shell != Some(true) {
            continue;
        }
        let cmd = trimmed
            .strip_prefix("$ ")
            .or_else(|| trimmed.strip_prefix("PS> "))
            .unwrap_or(trimmed);
        if !cmd.is_empty() && !cmd.starts_with('#') {
            out.push((i + 1, cmd.to_string()));
        }
    }
    out
}

/// `Some(Ok)` resolved, `Some(Err(why))` names something missing, `None`
/// not a command this checks.
fn resolve(cmd: &str, root: &Path, pkgs: &Targets) -> Option<Result<(), String>> {
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let flag = |f: &str| {
        words
            .iter()
            .position(|w| *w == f)
            .and_then(|i| words.get(i + 1))
            .copied()
    };
    match words.as_slice() {
        ["cargo", "xtask", sub, ..] => Some(
            xtask_commands()
                .any(|c| c == *sub)
                .then_some(())
                .ok_or(format!("no `cargo xtask {sub}`")),
        ),
        ["maya2c" | "./target/debug/maya2c", sub, ..] => Some(
            CLI_COMMANDS
                .contains(sub)
                .then_some(())
                .ok_or(format!("no `maya2c {sub}`")),
        ),
        ["cargo", ..] => {
            let pkg = flag("-p").or_else(|| flag("--package"))?;
            let Some(targets) = pkgs.get(pkg) else {
                return Some(Err(format!("no package `{pkg}`")));
            };
            for (flagname, kind) in [
                ("--test", "test"),
                ("--bin", "bin"),
                ("--example", "example"),
                ("--bench", "bench"),
            ] {
                if let Some(name) = flag(flagname)
                    && !targets.get(kind).is_some_and(|set| set.contains(name))
                {
                    return Some(Err(format!("`{pkg}` has no {kind} target `{name}`")));
                }
            }
            Some(Ok(()))
        }
        ["bash" | "sh" | "python" | "python3", path, ..] | [path, ..]
            if path.contains("scripts/") =>
        {
            let path = path.trim_start_matches("./");
            Some(
                root.join(path)
                    .exists()
                    .then_some(())
                    .ok_or(format!("no file `{path}`")),
            )
        }
        _ => None,
    }
}

/// Entry point.
///
/// # Errors
///
/// Any guide command that names something missing.
pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let pkgs = packages(&root)?;
    let mut files = vec![root.join("README.md")];
    let mut unreadable = Vec::new();
    markdown_files(&root.join("docs"), &mut files, &mut unreadable);
    let (resolved, unchecked, missing) = check(&files, &root, &pkgs, &mut unreadable);
    println!(
        "guides-check: {} files, {resolved} commands resolved, {} name something missing, {} files unreadable, {unchecked} not checkable here",
        files.len(),
        missing.len(),
        unreadable.len()
    );
    if missing.is_empty() && unreadable.is_empty() {
        return Ok(());
    }
    for m in unreadable.iter().chain(&missing) {
        eprintln!("{m}");
    }
    Err(format!(
        "{} guide commands or files failed ({} name something missing, {} unreadable)",
        missing.len() + unreadable.len(),
        missing.len(),
        unreadable.len()
    ))
}

/// `(resolved, unchecked, missing)` over `files`; a file that cannot be read
/// goes into `unreadable` rather than counting as checked.
fn check(
    files: &[PathBuf],
    root: &Path,
    pkgs: &Targets,
    unreadable: &mut Vec<String>,
) -> (usize, usize, Vec<String>) {
    let (mut resolved, mut unchecked, mut missing) = (0usize, 0usize, Vec::new());
    for file in files {
        let rel = file.strip_prefix(root).unwrap_or(file).display();
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(e) => {
                unreadable.push(format!("{rel}: cannot read: {e}"));
                continue;
            }
        };
        for (line, cmd) in commands(&text) {
            match resolve(&cmd, root, pkgs) {
                Some(Ok(())) => resolved += 1,
                Some(Err(why)) => missing.push(format!("{rel}:{line}: {why}\n    {cmd}")),
                None => unchecked += 1,
            }
        }
    }
    (resolved, unchecked, missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::unwrap_used)]
    fn an_unreadable_guide_or_directory_fails_rather_than_passing_unread() {
        let d = std::env::temp_dir().join(format!("guides-check-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // UTF-16 with a BOM: read_to_string refuses it, so its commands were never checked.
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(
                "```sh\ncargo test -p gone\n```\n"
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes),
            )
            .collect();
        std::fs::write(d.join("guide.md"), utf16).unwrap();

        let mut unreadable = Vec::new();
        let (resolved, _, missing) =
            check(&[d.join("guide.md")], &d, &Targets::new(), &mut unreadable);
        assert_eq!((resolved, missing.len()), (0, 0));
        assert_eq!(unreadable.len(), 1, "{unreadable:?}");
        assert!(
            unreadable[0].starts_with("guide.md: cannot read"),
            "{unreadable:?}"
        );

        let mut files = Vec::new();
        markdown_files(&d.join("no-such-dir"), &mut files, &mut unreadable);
        assert!(files.is_empty());
        assert!(unreadable[1].contains("cannot list"), "{unreadable:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn only_shell_blocks_are_read_and_prompts_are_stripped() {
        let doc = "```rust\ncargo nope\n```\n```sh\n$ cargo test -p x\n# comment\n```\n";
        assert_eq!(commands(doc), vec![(5, "cargo test -p x".to_string())]);
    }

    #[test]
    fn missing_things_are_named() {
        let mut pkgs = Targets::new();
        pkgs.entry("real".into())
            .or_default()
            .entry("test".into())
            .or_default()
            .insert("t".into());
        let root = Path::new(".");
        assert_eq!(
            resolve("cargo test -p real --test t", root, &pkgs),
            Some(Ok(()))
        );
        assert!(matches!(
            resolve("cargo test -p real --test gone", root, &pkgs),
            Some(Err(_))
        ));
        assert!(matches!(
            resolve("cargo test -p ghost", root, &pkgs),
            Some(Err(_))
        ));
        assert!(matches!(
            resolve("cargo xtask nope", root, &pkgs),
            Some(Err(_))
        ));
        assert_eq!(resolve("cargo xtask coverage", root, &pkgs), Some(Ok(())));
        assert_eq!(resolve("docker compose up", root, &pkgs), None);
    }

    #[test]
    fn every_dispatched_xtask_command_resolves() {
        let pkgs = Targets::new();
        let root = Path::new(".");
        for sub in ["bft-devnet", "proof-ratchet", "md-links", "devlog", "help"] {
            assert_eq!(
                resolve(&format!("cargo xtask {sub}"), root, &pkgs),
                Some(Ok(())),
                "{sub}"
            );
        }
    }
}
