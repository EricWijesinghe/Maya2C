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

/// `cargo xtask` commands, as `main.rs` dispatches them.
const XTASK_COMMANDS: &[&str] = &[
    "disk",
    "coverage",
    "release-check",
    "readiness",
    "spec-coverage",
    "slo-check",
    "go-no-go",
    "eco-metrics",
    "claims-check",
    "mesh-check",
    "sdk-e2e",
    "guides-check",
    "status",
    "sweep",
    "help",
];
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

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
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
                markdown_files(&path, out);
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
            XTASK_COMMANDS
                .contains(sub)
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
    markdown_files(&root.join("docs"), &mut files);
    let (mut resolved, mut unchecked, mut missing) = (0usize, 0usize, Vec::new());
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (line, cmd) in commands(&text) {
            match resolve(&cmd, &root, &pkgs) {
                Some(Ok(())) => resolved += 1,
                Some(Err(why)) => missing.push(format!(
                    "{}:{line}: {why}\n    {cmd}",
                    file.strip_prefix(&root).unwrap_or(file).display()
                )),
                None => unchecked += 1,
            }
        }
    }
    println!(
        "guides-check: {} files, {resolved} commands resolved, {} name something missing, {unchecked} not checkable here",
        files.len(),
        missing.len()
    );
    if missing.is_empty() {
        return Ok(());
    }
    for m in &missing {
        eprintln!("{m}");
    }
    Err(format!(
        "{} guide commands name something that does not exist",
        missing.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
