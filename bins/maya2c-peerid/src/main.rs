//! Prints the `PeerId` of a node identity, generating one if asked.
//!
//! ## Why this exists
//!
//! Cross-region bootnode multiaddrs name the peer they expect:
//!
//! ```text
//! /dns4/seed-0-eu.p2p.example.com/tcp/30333/p2p/12D3KooW...
//! ```
//!
//! Without the `/p2p/` component a dial cannot be authenticated — the node
//! connects to whatever answers on that address and learns its identity
//! afterwards, which is exactly the opening a DNS hijack needs. But the `PeerId`
//! is derived from a key the pod would normally create on its own first boot,
//! long after the manifests naming it were written.
//!
//! This breaks that circle. An operator generates the nine seed identities up
//! front, reads their `PeerId`s here, writes them into the overlays, and ships
//! the keys as Secrets.
//!
//! ```text
//! # Generate a seed identity and print who it will be.
//! peerid --data-dir ./seeds/eu-0 --create
//!
//! # Read the identity of a key that already exists.
//! peerid --key ./seeds/eu-0/node_key
//! ```

use std::error::Error;
use std::path::PathBuf;

use custom_l1_node::network::identity;
use libp2p::PeerId;

/// Parsed command line.
#[derive(Debug)]
struct Args {
    /// Data directory holding `node_key`, when `--key` was not given.
    data_dir: Option<PathBuf>,
    /// Explicit key file.
    key: Option<PathBuf>,
    /// Whether a missing key may be created.
    create: bool,
}

fn print_usage() {
    println!(
        "peerid — report the PeerId of a node identity\n\n\
         USAGE:\n  \
         peerid --key <PATH>\n  \
         peerid --data-dir <PATH> [--create]\n\n\
         OPTIONS:\n  \
         --key <PATH>       read this key file directly\n  \
         --data-dir <PATH>  read <PATH>/node_key\n  \
         --create           generate the key if it does not exist; requires --data-dir\n  \
         -h, --help         show this message\n\n\
         Prints the PeerId on stdout and nothing else, so it can be substituted\n\
         straight into a bootnode multiaddr."
    );
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut data_dir = None;
    let mut key = None;
    let mut create = false;

    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let mut value = || {
            argv.next()
                .ok_or_else(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--data-dir" => data_dir = Some(PathBuf::from(value()?)),
            "--key" => key = Some(PathBuf::from(value()?)),
            "--create" => create = true,
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}").into()),
        }
    }

    Ok(Args {
        data_dir,
        key,
        create,
    })
}

/// Resolves the arguments to a `PeerId`.
///
/// Kept separate from `main` so the argument combinations are testable without
/// spawning a process.
fn resolve(args: &Args) -> Result<PeerId, Box<dyn Error>> {
    match (&args.key, &args.data_dir) {
        (Some(_), Some(_)) => Err("--key and --data-dir are mutually exclusive; pass one".into()),
        (None, None) => Err("one of --key or --data-dir is required".into()),

        // An explicit key file is read, never written. Creating a key at a path
        // the operator named exactly is how a typo becomes a new identity that
        // no bootnode list knows about.
        (Some(path), None) => {
            if args.create {
                return Err("--create requires --data-dir, not --key".into());
            }
            Ok(identity::peer_id_at(path)?)
        }

        (None, Some(dir)) => {
            if args.create {
                let keypair = identity::load_or_create(dir)?;
                return Ok(PeerId::from(keypair.public()));
            }
            Ok(identity::peer_id_at(&identity::key_path(dir))?)
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("peerid: {error}");
            print_usage();
            std::process::exit(2);
        }
    };

    match resolve(&args) {
        // Bare, newline-terminated, nothing else on stdout: the output is meant
        // to be captured by `$(...)` and pasted into a multiaddr.
        Ok(peer_id) => println!("{peer_id}"),
        Err(error) => {
            eprintln!("peerid: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn args(data_dir: Option<&str>, key: Option<&str>, create: bool) -> Args {
        Args {
            data_dir: data_dir.map(PathBuf::from),
            key: key.map(PathBuf::from),
            create,
        }
    }

    #[test]
    fn creating_then_reading_yields_the_same_peer_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_str().expect("utf-8 path");

        let created = resolve(&args(Some(path), None, true)).expect("create");
        let reread = resolve(&args(Some(path), None, false)).expect("reread");

        assert_eq!(created, reread);
    }

    #[test]
    fn reading_a_missing_key_fails_rather_than_inventing_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_str().expect("utf-8 path");

        assert!(resolve(&args(Some(path), None, false)).is_err());
    }

    #[test]
    fn an_explicit_key_path_is_never_created() {
        // Otherwise a mistyped path silently mints a fresh identity, and the
        // seed it was meant to describe stays undialable.
        let dir = tempfile::tempdir().expect("tempdir");
        let key = dir.path().join("typo_key");

        let error = resolve(&args(None, key.to_str(), true)).expect_err("must refuse");
        assert!(error.to_string().contains("--create requires --data-dir"));
        assert!(!key.exists(), "created a key for an explicit --key path");
    }

    #[test]
    fn the_two_source_flags_are_mutually_exclusive() {
        assert!(resolve(&args(Some("a"), Some("b"), false)).is_err());
    }

    #[test]
    fn one_source_flag_is_required() {
        assert!(resolve(&args(None, None, false)).is_err());
    }
}
