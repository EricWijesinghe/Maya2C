//! Faucet rate limiting: one grant per day, per IP **and** per address.
//!
//! # Why not one key per (IP, address) pair
//!
//! Because that is not a limit.
//!
//! Keying on the pair means the bucket is `(ip, address)`. Generating an
//! ML-DSA keypair is free and unbounded — `generate_signing_key()` in a loop —
//! so one IP with a thousand fresh addresses is a thousand distinct pairs and a
//! thousand payouts. The mirror image holds too: one address behind a thousand
//! proxies is a thousand pairs.
//!
//! So the two buckets are **independent**, and a request must clear both. An
//! attacker who owns one IP is limited by that IP no matter how many keys they
//! make; an attacker with a thousand IPs is still limited per address if they
//! reuse one.
//!
//! # What this does not stop
//!
//! A thousand IPs *and* a thousand fresh addresses. That is a thousand
//! legitimate-looking requests and no per-key limiter can see it — the requests
//! are indistinguishable from a thousand real users. Defending it needs
//! something outside this module: proof of work on the request, an
//! authenticated account, or a funding cap that bounds the loss regardless of
//! who is asking.
//!
//! [`Faucet`](crate::Faucet) has that cap for exactly this reason. The limiter bounds one
//! actor; the cap bounds the day.
//!
//! # Check both, then commit both
//!
//! Subtle and worth stating: the buckets are consumed only if **both** pass.
//!
//! Consuming the IP bucket before discovering the address is blocked would
//! spend a user's daily allowance on a request that was refused — so somebody
//! who mistypes an address that was already funded loses their own access for a
//! day, through no fault of theirs. `check_and_commit` therefore tests both
//! first and writes both after.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, SystemTime};

/// How long one grant blocks the next, for both keys.
pub const WINDOW: Duration = Duration::from_secs(24 * 60 * 60);

/// Distinct IPs and addresses tracked before a sweep.
///
/// The tables are the memory an attacker grows by varying their key, so they
/// need a ceiling. Entries older than [`WINDOW`] are dropped when it is
/// reached: an expired entry and an absent one produce the same answer, so
/// forgetting it changes no decision.
pub const MAX_TRACKED: usize = 262_144;

/// Why a request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// This IP was granted within the window.
    Ip {
        /// Seconds until it may ask again.
        retry_after: u64,
    },
    /// This address was granted within the window.
    ///
    /// Distinct from [`Refusal::Ip`] because the remedy differs: a user behind
    /// a shared NAT hitting the IP limit is not doing anything wrong, and
    /// telling them "this address already has funds" would be a lie.
    Address {
        /// Seconds until it may be funded again.
        retry_after: u64,
    },
}

impl Refusal {
    /// Seconds the caller should wait.
    #[must_use]
    pub const fn retry_after(self) -> u64 {
        match self {
            Self::Ip { retry_after } | Self::Address { retry_after } => retry_after,
        }
    }

    /// A message that does not confuse one limit for the other.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Ip { .. } => {
                "This network address has already requested funds today. If you are \
                 behind a shared connection, someone else may have used it."
            }
            Self::Address { .. } => {
                "This wallet address has already been funded today. A different \
                 address will not help from the same connection."
            }
        }
    }
}

/// Independent per-IP and per-address windows.
#[derive(Debug, Default)]
pub struct Limiter {
    ips: HashMap<IpAddr, SystemTime>,
    addresses: HashMap<[u8; 32], SystemTime>,
}

impl Limiter {
    /// A limiter with no history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a grant would be permitted, without recording one.
    ///
    /// For a `GET`-style check that a UI can call before showing a form. It
    /// deliberately does not commit, so polling it cannot exhaust anything.
    #[must_use]
    pub fn would_permit(&self, ip: IpAddr, address: &[u8; 32], now: SystemTime) -> Option<Refusal> {
        if let Some(retry_after) = remaining(self.ips.get(&ip), now) {
            return Some(Refusal::Ip { retry_after });
        }
        if let Some(retry_after) = remaining(self.addresses.get(address), now) {
            return Some(Refusal::Address { retry_after });
        }
        None
    }

    /// Records a grant if both windows are clear.
    ///
    /// Both are tested before either is written — see the module
    /// documentation. A refusal consumes nothing.
    ///
    /// # Errors
    ///
    /// The [`Refusal`] naming which limit was hit, and for how long.
    pub fn check_and_commit(
        &mut self,
        ip: IpAddr,
        address: &[u8; 32],
        now: SystemTime,
    ) -> Result<(), Refusal> {
        if let Some(refusal) = self.would_permit(ip, address, now) {
            return Err(refusal);
        }

        if self.ips.len() >= MAX_TRACKED || self.addresses.len() >= MAX_TRACKED {
            self.sweep(now);
        }

        self.ips.insert(ip, now);
        self.addresses.insert(*address, now);
        Ok(())
    }

    /// Restores a grant made at `at`, read back from the journal. Keeps the
    /// later of two times for one key, so replay order does not matter.
    pub fn record(&mut self, ip: IpAddr, address: &[u8; 32], at: SystemTime) {
        let later = |old: &mut SystemTime| *old = (*old).max(at);
        self.ips.entry(ip).and_modify(later).or_insert(at);
        self.addresses
            .entry(*address)
            .and_modify(later)
            .or_insert(at);
    }

    /// Drops entries whose window has elapsed.
    pub fn sweep(&mut self, now: SystemTime) {
        self.ips.retain(|_, at| remaining(Some(at), now).is_some());
        self.addresses
            .retain(|_, at| remaining(Some(at), now).is_some());
    }

    /// IPs currently held.
    #[must_use]
    pub fn tracked_ips(&self) -> usize {
        self.ips.len()
    }

    /// Addresses currently held.
    #[must_use]
    pub fn tracked_addresses(&self) -> usize {
        self.addresses.len()
    }
}

/// Seconds left in the window, or `None` if it has elapsed.
///
/// `duration_since` on a clock that went backwards yields an error rather than
/// a negative, and the safe reading of "I cannot tell how long ago this was" is
/// that the window is still open — refusing is recoverable in a day, granting
/// twice is not.
fn remaining(granted_at: Option<&SystemTime>, now: SystemTime) -> Option<u64> {
    let granted_at = granted_at?;
    match now.duration_since(*granted_at) {
        Ok(elapsed) if elapsed >= WINDOW => None,
        Ok(elapsed) => Some(WINDOW.saturating_sub(elapsed).as_secs()),
        Err(_) => Some(WINDOW.as_secs()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    fn address(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600)
    }

    #[test]
    fn a_first_request_is_granted() {
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(1), now())
            .expect("a fresh pair is granted");
    }

    #[test]
    fn one_ip_cannot_drain_the_faucet_with_fresh_addresses() {
        // The attack a pair-keyed limiter permits, and the reason this module
        // exists. Generating ML-DSA keypairs is free; if the address were part
        // of the key, this loop would succeed a thousand times.
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(0), now())
            .expect("first request");

        for byte in 1..=255u8 {
            let refusal = limiter
                .check_and_commit(ip(1), &address(byte), now())
                .expect_err("a fresh address must not reset the IP window");
            assert!(matches!(refusal, Refusal::Ip { .. }));
        }
    }

    #[test]
    fn one_address_cannot_be_funded_from_many_ips() {
        // The mirror image: a thousand proxies do not make one address
        // eligible a thousand times.
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(7), now())
            .expect("first request");

        for last in 2..=255u8 {
            let refusal = limiter
                .check_and_commit(ip(last), &address(7), now())
                .expect_err("a fresh IP must not reset the address window");
            assert!(matches!(refusal, Refusal::Address { .. }));
        }
    }

    #[test]
    fn a_refusal_consumes_nothing() {
        // Check both, then commit both. If the IP bucket were written before
        // the address check, this second IP would be burned by a request that
        // was refused — and a user who mistyped an already-funded address
        // would lose their own access for a day.
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(7), now())
            .expect("first request");

        // ip(2) is fresh; address(7) is not. The request is refused.
        limiter
            .check_and_commit(ip(2), &address(7), now())
            .expect_err("the address is spent");

        // ip(2) must still be usable with a different address.
        limiter
            .check_and_commit(ip(2), &address(8), now())
            .expect("a refused request must not have consumed the IP");
    }

    #[test]
    fn the_window_expires() {
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(1), now())
            .expect("first");

        let almost = now() + WINDOW - Duration::from_secs(1);
        assert!(
            limiter
                .check_and_commit(ip(1), &address(1), almost)
                .is_err()
        );

        let after = now() + WINDOW;
        limiter
            .check_and_commit(ip(1), &address(1), after)
            .expect("the window has elapsed");
    }

    #[test]
    fn a_refusal_says_which_limit_and_for_how_long() {
        // The two refusals need different remedies, so they must be
        // distinguishable — telling a user behind a shared NAT that "this
        // address already has funds" would be false.
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(1), now())
            .expect("first");

        let later = now() + Duration::from_secs(3_600);

        let by_ip = limiter
            .check_and_commit(ip(1), &address(2), later)
            .expect_err("ip");
        assert!(matches!(by_ip, Refusal::Ip { .. }));
        assert!(by_ip.message().contains("network address"));
        assert_eq!(by_ip.retry_after(), WINDOW.as_secs() - 3_600);

        let by_address = limiter
            .check_and_commit(ip(2), &address(1), later)
            .expect_err("address");
        assert!(matches!(by_address, Refusal::Address { .. }));
        assert!(by_address.message().contains("wallet address"));
    }

    #[test]
    fn the_ip_limit_is_reported_before_the_address_limit() {
        // Both exhausted: the order is fixed so two faucet instances give one
        // answer, and any log or ban score keyed on it agrees.
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(1), now())
            .expect("first");

        let refusal = limiter
            .check_and_commit(ip(1), &address(1), now())
            .expect_err("both are spent");
        assert!(matches!(refusal, Refusal::Ip { .. }));
    }

    #[test]
    fn would_permit_does_not_consume() {
        // A UI polling "can I ask?" must not spend the allowance it is asking
        // about.
        let limiter = Limiter::new();
        for _ in 0..100 {
            assert!(limiter.would_permit(ip(1), &address(1), now()).is_none());
        }
        assert_eq!(limiter.tracked_ips(), 0);
    }

    #[test]
    fn expired_entries_are_swept() {
        let mut limiter = Limiter::new();
        for last in 0..=255u8 {
            limiter
                .check_and_commit(ip(last), &address(last), now())
                .expect("granted");
        }
        assert_eq!(limiter.tracked_ips(), 256);

        limiter.sweep(now() + WINDOW);
        assert_eq!(
            limiter.tracked_ips(),
            0,
            "elapsed windows carry no information"
        );
        assert_eq!(limiter.tracked_addresses(), 0);
    }

    #[test]
    fn a_backwards_clock_refuses_rather_than_granting_twice() {
        // NTP steps happen. Refusing is recoverable in a day; granting twice
        // is not recoverable at all.
        let mut limiter = Limiter::new();
        limiter
            .check_and_commit(ip(1), &address(1), now())
            .expect("first");

        let earlier = now() - Duration::from_secs(3_600);
        assert!(
            limiter
                .check_and_commit(ip(1), &address(1), earlier)
                .is_err(),
            "a clock that went backwards must not open the window"
        );
    }
}
