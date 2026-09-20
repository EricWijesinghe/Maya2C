//! Workspace automation.
//!
//! Two commands, both of which exist because something went wrong without
//! them:
//!
//! - `disk` — `target/` reached 415 GB once and filled the volume to exactly
//!   zero bytes, at which point every build fails with `os error 112`, which
//!   names nothing about the real problem. It was 289 GB at the commit that
//!   added this crate.
//! - `coverage` — subsystem status lived in three prose documents and nothing
//!   checked that a subsystem claiming to work had a test. `features.toml` is
//!   the machine-readable half; this is what reads it.
//!
//! Run as `cargo xtask <command>` (the alias is in `.cargo/config.toml`).

mod coverage;
mod disk;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };

    let result = match cmd {
        "disk" => disk::run(rest),
        "coverage" => coverage::run(rest),
        "help" | "--help" | "-h" => {
            usage();
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown command `{other}`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "\
cargo xtask <command>

  disk [--check]        Report build-artifact size. `--check` exits non-zero
                        above the ceiling in xtask/src/disk.rs.
  coverage [--quiet]    Read features.toml, print the ledger, and fail if an
                        entry claims `working` or `verified` without naming a
                        test that exists."
    );
}

/// The workspace root, found by walking up from this crate's manifest.
///
/// `CARGO_MANIFEST_DIR` is set by cargo when it builds this binary, so the
/// answer does not depend on the directory the command was run from.
pub fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}
