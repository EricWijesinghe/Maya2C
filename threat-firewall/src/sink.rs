//! Where a decision becomes a rule.
//!
//! Commands are built as argument vectors from a parsed [`IpAddr`] and a table
//! name checked at configuration, and run without a shell, so nothing a peer
//! controls reaches a command line as text.

use std::collections::BTreeSet;
use std::net::IpAddr;
use std::process::Command;

use crate::error::{FirewallError, Result};

/// One change to the ruleset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkAction {
    /// Start dropping traffic from an address.
    Block(IpAddr),
    /// Stop dropping it.
    Unblock(IpAddr),
}

/// Applies rules. A failure leaves the rule unenforced; the worker retries it.
pub trait FirewallSink: Send {
    /// Applies one change.
    ///
    /// # Errors
    ///
    /// [`FirewallError::Sink`] if the change could not be made.
    fn apply(&mut self, action: SinkAction) -> Result<()>;
}

/// Records what it would have done. For tests and for a first run on a new
/// host.
#[derive(Clone, Debug, Default)]
pub struct DryRunSink {
    /// Every action, in order.
    pub log: Vec<SinkAction>,
    /// What would be blocked now.
    pub blocked: BTreeSet<IpAddr>,
}

impl FirewallSink for DryRunSink {
    fn apply(&mut self, action: SinkAction) -> Result<()> {
        match action {
            SinkAction::Block(ip) => self.blocked.insert(ip),
            SinkAction::Unblock(ip) => self.blocked.remove(&ip),
        };
        self.log.push(action);
        Ok(())
    }
}

/// Which netfilter front end to drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Elements of the `threat4` / `threat6` sets in an `inet` table the
    /// operator created, with a drop rule matching them — see
    /// `docs/threat-intel.md`. Adding an element twice is not an error.
    Nftables {
        /// The `inet` table's name.
        table: String,
    },
    /// One `INPUT` drop rule per address, tagged with a comment, via
    /// `iptables` / `ip6tables`. Checked with `-C` before inserting, so a
    /// restarted worker does not stack duplicates.
    Iptables,
}

/// Comment tagging every iptables rule this worker owns.
pub const IPTABLES_COMMENT: &str = "maya2c-threat";

/// One program run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// The executable.
    pub program: &'static str,
    /// Its arguments.
    pub args: Vec<String>,
}

impl Backend {
    /// An nftables backend over table `table`.
    ///
    /// # Errors
    ///
    /// [`FirewallError::Config`] unless the name is 1–32 characters of
    /// `[a-z0-9_]`.
    pub fn nftables(table: &str) -> Result<Self> {
        let valid = (1..=32).contains(&table.len())
            && table
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !valid {
            return Err(FirewallError::Config(format!(
                "nftables table name {table:?} must be 1-32 of [a-z0-9_]"
            )));
        }
        Ok(Self::Nftables {
            table: table.to_owned(),
        })
    }

    /// The command that makes `action` true.
    #[must_use]
    pub fn change(&self, action: SinkAction) -> Invocation {
        let (ip, adding) = match action {
            SinkAction::Block(ip) => (ip, true),
            SinkAction::Unblock(ip) => (ip, false),
        };
        match self {
            Self::Nftables { table } => {
                let set = if ip.is_ipv4() { "threat4" } else { "threat6" };
                let verb = if adding { "add" } else { "delete" };
                Invocation {
                    program: "nft",
                    args: [
                        verb,
                        "element",
                        "inet",
                        table,
                        set,
                        "{",
                        &ip.to_string(),
                        "}",
                    ]
                    .map(str::to_owned)
                    .to_vec(),
                }
            }
            Self::Iptables => iptables(ip, if adding { "-I" } else { "-D" }),
        }
    }

    /// A command whose success means `action` is already true, if this backend
    /// has one.
    #[must_use]
    pub fn check(&self, action: SinkAction) -> Option<Invocation> {
        match (self, action) {
            (Self::Iptables, SinkAction::Block(ip)) => Some(iptables(ip, "-C")),
            _ => None,
        }
    }
}

fn iptables(ip: IpAddr, verb: &str) -> Invocation {
    Invocation {
        program: if ip.is_ipv4() {
            "iptables"
        } else {
            "ip6tables"
        },
        args: [
            verb,
            "INPUT",
            "-s",
            &ip.to_string(),
            "-m",
            "comment",
            "--comment",
            IPTABLES_COMMENT,
            "-j",
            "DROP",
        ]
        .map(str::to_owned)
        .to_vec(),
    }
}

/// Runs a [`Backend`]'s commands on this host. Needs `CAP_NET_ADMIN`, and
/// nothing else: the worker holds no chain key.
#[derive(Clone, Debug)]
pub struct CommandSink {
    backend: Backend,
}

impl CommandSink {
    /// A sink driving `backend`.
    #[must_use]
    pub const fn new(backend: Backend) -> Self {
        Self { backend }
    }
}

impl FirewallSink for CommandSink {
    fn apply(&mut self, action: SinkAction) -> Result<()> {
        if let Some(check) = self.backend.check(action)
            && run(&check).is_ok()
        {
            return Ok(());
        }
        run(&self.backend.change(action))
    }
}

fn run(invocation: &Invocation) -> Result<()> {
    let status = Command::new(invocation.program)
        .args(&invocation.args)
        .status()
        .map_err(|e| FirewallError::Sink(format!("{}: {e}", invocation.program)))?;
    if status.success() {
        Ok(())
    } else {
        Err(FirewallError::Sink(format!(
            "{} {} exited with {status}",
            invocation.program,
            invocation.args.join(" ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    const V4: IpAddr = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7));
    const V6: IpAddr = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 7));

    #[test]
    fn nftables_commands_name_the_family_set() {
        let backend = Backend::nftables("maya2c").expect("valid");
        assert_eq!(
            backend.change(SinkAction::Block(V4)).args,
            [
                "add",
                "element",
                "inet",
                "maya2c",
                "threat4",
                "{",
                "198.51.100.7",
                "}"
            ]
        );
        let unblock = backend.change(SinkAction::Unblock(V6));
        assert_eq!(unblock.program, "nft");
        assert_eq!(unblock.args[0], "delete");
        assert_eq!(unblock.args[4], "threat6");
        assert_eq!(backend.check(SinkAction::Block(V4)), None);
    }

    #[test]
    fn iptables_checks_before_it_inserts_and_picks_the_family_binary() {
        let block = SinkAction::Block(V6);
        let check = Backend::Iptables.check(block).expect("a check");
        assert_eq!(check.program, "ip6tables");
        assert_eq!(check.args[0], "-C");
        let insert = Backend::Iptables.change(SinkAction::Block(V4));
        assert_eq!(insert.program, "iptables");
        assert_eq!(
            insert.args,
            [
                "-I",
                "INPUT",
                "-s",
                "198.51.100.7",
                "-m",
                "comment",
                "--comment",
                IPTABLES_COMMENT,
                "-j",
                "DROP"
            ]
        );
        assert_eq!(Backend::Iptables.check(SinkAction::Unblock(V4)), None);
    }

    #[test]
    fn a_table_name_that_could_carry_syntax_is_refused() {
        for bad in ["", "Maya", "maya; flush ruleset", "a b", &"x".repeat(33)] {
            assert!(Backend::nftables(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_dry_run_tracks_what_would_be_blocked() {
        let mut sink = DryRunSink::default();
        sink.apply(SinkAction::Block(V4)).expect("dry");
        sink.apply(SinkAction::Block(V6)).expect("dry");
        sink.apply(SinkAction::Unblock(V4)).expect("dry");
        assert_eq!(sink.blocked, BTreeSet::from([V6]));
        assert_eq!(sink.log.len(), 3);
    }
}
