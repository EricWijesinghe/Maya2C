//! The wallet flows as state graphs (Master Prompt 29 §2).
//!
//! Each flow starts at `start` and ends at `done` or `cancelled`. [`check`]
//! enforces the "no dead ends" rule mechanically:
//!
//! 1. every state is reachable from `start`;
//! 2. `done` is reachable from every state except `cancelled` — including
//!    from `error`, so an error is always recoverable;
//! 3. `cancelled` is reachable from every state except `done`, or the state
//!    leads straight to `done` — the user can always leave, except from a
//!    screen that only reports an outcome already committed on-chain;
//! 4. every flow has an `error` state.
//!
//! The screens implement these graphs; the graphs are the specification the
//! screens are reviewed against.

/// One flow.
#[derive(Debug)]
pub struct Flow {
    /// Flow name.
    pub name: &'static str,
    /// Directed edges between named states.
    pub edges: &'static [(&'static str, &'static str)],
}

macro_rules! flow {
    ($name:literal: $($a:literal -> $b:literal),* $(,)?) => {
        Flow { name: $name, edges: &[$(($a, $b)),*] }
    };
}

/// Every wallet flow.
pub const FLOWS: &[Flow] = &[
    // Onboarding without a seed phrase: a device passkey plus recovery
    // guardians (the Master Prompt 22 account model).
    flow!("onboarding":
        "start" -> "create_passkey", "create_passkey" -> "choose_guardians",
        "choose_guardians" -> "confirm", "confirm" -> "done",
        "create_passkey" -> "error", "confirm" -> "error", "error" -> "create_passkey",
        "start" -> "cancelled", "create_passkey" -> "cancelled",
        "choose_guardians" -> "cancelled", "confirm" -> "cancelled", "error" -> "cancelled"),
    flow!("receive":
        "start" -> "show_address", "show_address" -> "done", "start" -> "error",
        "error" -> "start", "error" -> "cancelled", "show_address" -> "cancelled",
        "start" -> "cancelled"),
    flow!("send":
        "start" -> "enter_recipient", "enter_recipient" -> "check_recipient",
        "check_recipient" -> "enter_amount", "check_recipient" -> "warn_poisoning",
        "warn_poisoning" -> "enter_recipient", "warn_poisoning" -> "enter_amount",
        "enter_amount" -> "review", "review" -> "sign", "sign" -> "submitted",
        "submitted" -> "done", "sign" -> "error", "submitted" -> "error",
        "error" -> "review", "error" -> "cancelled",
        "start" -> "cancelled", "enter_recipient" -> "cancelled",
        "check_recipient" -> "cancelled", "warn_poisoning" -> "cancelled",
        "enter_amount" -> "cancelled", "review" -> "cancelled", "sign" -> "cancelled"),
    flow!("swap":
        "start" -> "pick_pair", "pick_pair" -> "check_token", "check_token" -> "quote",
        "check_token" -> "warn_lookalike", "warn_lookalike" -> "pick_pair",
        "quote" -> "review", "review" -> "sign", "sign" -> "done",
        "quote" -> "error", "sign" -> "error", "error" -> "quote", "error" -> "cancelled",
        "start" -> "cancelled", "pick_pair" -> "cancelled", "check_token" -> "cancelled",
        "warn_lookalike" -> "cancelled", "quote" -> "cancelled", "review" -> "cancelled"),
    // A cross-chain intent can expire unfilled; the refund path is the recovery.
    flow!("intent":
        "start" -> "describe", "describe" -> "review", "review" -> "sign",
        "sign" -> "pending_fill", "pending_fill" -> "done", "pending_fill" -> "expired",
        "expired" -> "refunded", "refunded" -> "done", "sign" -> "error",
        "error" -> "review", "error" -> "cancelled",
        "start" -> "cancelled", "describe" -> "cancelled", "review" -> "cancelled",
        "expired" -> "cancelled"),
    flow!("recovery":
        "start" -> "request", "request" -> "await_guardians", "await_guardians" -> "timelock",
        "timelock" -> "new_passkey", "new_passkey" -> "done", "await_guardians" -> "error",
        "error" -> "request", "error" -> "cancelled",
        "start" -> "cancelled", "request" -> "cancelled", "await_guardians" -> "cancelled",
        "timelock" -> "cancelled"),
    flow!("vault_settings":
        "start" -> "edit_limits", "edit_limits" -> "review", "review" -> "sign",
        "sign" -> "timelocked", "timelocked" -> "done", "sign" -> "error",
        "error" -> "review", "error" -> "cancelled", "timelocked" -> "cancelled",
        "start" -> "cancelled", "edit_limits" -> "cancelled", "review" -> "cancelled"),
    flow!("app_connection":
        "start" -> "check_domain", "check_domain" -> "show_permissions",
        "check_domain" -> "warn_phishing", "warn_phishing" -> "cancelled",
        "warn_phishing" -> "show_permissions", "show_permissions" -> "done",
        "start" -> "error", "error" -> "start", "error" -> "cancelled",
        "start" -> "cancelled", "check_domain" -> "cancelled",
        "show_permissions" -> "cancelled"),
    // The Master Prompt 22 clear-signing review: a flagged request can only
    // be signed after the warning is acknowledged.
    flow!("clear_sign_review":
        "start" -> "decode", "decode" -> "summary", "decode" -> "error",
        "summary" -> "flagged", "flagged" -> "acknowledge", "acknowledge" -> "sign",
        "summary" -> "sign", "sign" -> "done", "error" -> "decode", "error" -> "cancelled",
        "start" -> "cancelled", "decode" -> "cancelled", "summary" -> "cancelled",
        "flagged" -> "cancelled", "acknowledge" -> "cancelled", "sign" -> "cancelled"),
    flow!("approvals":
        "start" -> "list", "list" -> "select", "select" -> "revoke_sign",
        "revoke_sign" -> "done", "list" -> "done", "revoke_sign" -> "error",
        "error" -> "select", "error" -> "cancelled",
        "start" -> "cancelled", "list" -> "cancelled", "select" -> "cancelled"),
];

fn states(f: &Flow) -> Vec<&'static str> {
    let mut v: Vec<_> = f.edges.iter().flat_map(|&(a, b)| [a, b]).collect();
    v.sort_unstable();
    v.dedup();
    v
}

fn reaches(f: &Flow, from: &str, to: &str) -> bool {
    let mut seen = vec![from];
    let mut i = 0;
    while let Some(&s) = seen.get(i) {
        if s == to {
            return true;
        }
        for &(a, b) in f.edges {
            if a == s && !seen.contains(&b) {
                seen.push(b);
            }
        }
        i += 1;
    }
    false
}

/// Every violation of the four rules, as readable strings; empty means sound.
#[must_use]
pub fn check(f: &Flow) -> Vec<String> {
    let all = states(f);
    let mut out = Vec::new();
    if !all.contains(&"error") {
        out.push(format!("{}: no error state", f.name));
    }
    for s in all {
        if !reaches(f, "start", s) {
            out.push(format!("{}: `{s}` is unreachable", f.name));
        }
        if s != "cancelled" && !reaches(f, s, "done") {
            out.push(format!("{}: `{s}` cannot reach done (dead end)", f.name));
        }
        let finishing = f.edges.contains(&(s, "done"));
        if s != "done" && !finishing && !reaches(f, s, "cancelled") {
            out.push(format!("{}: `{s}` cannot leave (no cancel)", f.name));
        }
    }
    out
}
