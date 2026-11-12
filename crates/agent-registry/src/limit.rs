//! Per-client request budget for the HTTP registry.

use std::collections::BTreeMap;
use std::net::IpAddr;

/// Most distinct clients counted in one window. A new client past this is
/// refused until the window rolls over: the limiter fails closed rather than
/// growing without bound.
pub const MAX_CLIENTS: usize = 65_536;

/// The key a client is counted under. IPv6 counts per /64, the block one
/// subscriber is normally given; per address, one host could rotate through
/// 2^64 keys and never be limited.
pub fn client_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let s = v6.segments();
                format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
            }
        },
    }
}

/// Fixed-window counter per client key. Every key shares one window, so the
/// whole table is dropped when the window rolls over: memory is bounded by
/// `MAX_CLIENTS` entries, never by the process's lifetime.
#[derive(Debug)]
pub struct FixedWindow {
    window_secs: u64,
    max: u32,
    window_start: u64,
    /// key -> requests admitted in the current window
    counts: BTreeMap<String, u32>,
}

impl FixedWindow {
    pub fn new(window_secs: u64, max: u32) -> Self {
        Self {
            window_secs: window_secs.max(1),
            max,
            window_start: 0,
            counts: BTreeMap::new(),
        }
    }

    /// Counts one request from `key` at `now` (Unix seconds). False when over budget.
    pub fn admit(&mut self, key: &str, now: u64) -> bool {
        let start = now - now % self.window_secs;
        if start != self.window_start {
            self.window_start = start;
            self.counts.clear();
        }
        if !self.counts.contains_key(key) && self.counts.len() >= MAX_CLIENTS {
            return false;
        }
        let count = self.counts.entry(key.to_owned()).or_insert(0);
        if *count >= self.max {
            return false;
        }
        *count += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_clients_in_one_slash_64_share_a_budget() {
        let a: IpAddr = "2001:db8:1:2::1".parse().expect("ip");
        let b: IpAddr = "2001:db8:1:2:ffff::9".parse().expect("ip");
        let c: IpAddr = "2001:db8:1:3::1".parse().expect("ip");
        assert_eq!(client_key(a), client_key(b));
        assert_ne!(client_key(a), client_key(c));
    }

    #[test]
    fn a_full_table_refuses_new_clients_until_the_window_rolls() {
        let mut limiter = FixedWindow::new(60, 5);
        for i in 0..MAX_CLIENTS {
            assert!(limiter.admit(&i.to_string(), 0));
        }
        assert!(!limiter.admit("newcomer", 1));
        // A client already counted keeps its budget.
        assert!(limiter.admit("0", 1));
        assert!(limiter.admit("newcomer", 60));
    }
}
