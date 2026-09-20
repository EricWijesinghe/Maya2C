//! A rate-limited testnet faucet.
//!
//! # A faucet is a hot wallet with a public endpoint
//!
//! That is the whole security story, and it is worth saying before any of the
//! mechanism. This service holds a funded signing key and hands value to
//! anyone who asks. Every control here exists to bound what happens when — not
//! if — somebody tries to drain it.
//!
//! Three bounds, at different scopes:
//!
//! | Control | Bounds |
//! |---|---|
//! | [`limit::Limiter`] | one actor, per IP and per address independently |
//! | [`Faucet::daily_cap`] | the whole day, regardless of who is asking |
//! | [`Faucet::new`] | value-bearing chains, refused outright |
//!
//! The limiter is the one people think of and the weakest of the three. A
//! thousand IPs with a thousand fresh addresses is a thousand
//! indistinguishable, legitimate-looking requests, and no per-key limiter can
//! see that. The **cap** is what makes that attack cost the faucet a known,
//! chosen amount rather than everything.
//!
//! # It refuses value-bearing chains
//!
//! The sixth guard, alongside `SETUP_IS_TRUSTED`, the node's startup check,
//! both terraform module sets, the genesis ceremony, and the wallet's shielded
//! composer. A faucet that dispensed on mainnet would be handing out real value
//! from a key sitting in a web service.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod dispense;
pub mod http;
pub mod limit;
pub mod testing;

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::limit::{Limiter, Refusal};

/// Chain ids on which value is real and a faucet must not run.
///
/// Repeated rather than shared, like the node's and the ceremony's copies:
/// each answers a different question, and one list would invite relaxing all
/// of them together.
const VALUE_BEARING_CHAINS: &[&str] = &["maya-mainnet", "mainnet"];

/// Why a faucet request was not fulfilled.
#[derive(Debug, thiserror::Error)]
pub enum FaucetError {
    /// A rate limit was hit.
    #[error("{}", .0.message())]
    RateLimited(Refusal),

    /// The day's budget is spent.
    ///
    /// Distinct from a rate limit: nothing the caller does makes this clear
    /// sooner, and telling them to "try a different address" would be wrong.
    #[error("the faucet's daily budget is exhausted; it refills at midnight UTC")]
    BudgetExhausted,

    /// The address was not 32 hex-encoded bytes.
    #[error("address must be 64 hex characters: {0}")]
    BadAddress(String),

    /// The faucet is configured for a chain where value is real.
    #[error("faucet disabled: {0}")]
    ValueBearingChain(String),
}

/// A grant.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Grant {
    /// Units dispensed.
    pub amount: u64,
    /// Units left in today's budget after this grant.
    pub remaining_today: u64,
    /// Seconds until this caller may ask again.
    pub next_request_in: u64,
}

/// The faucet's policy and state.
#[derive(Debug)]
pub struct Faucet {
    chain_id: String,
    dispense: u64,
    daily_cap: u64,
    /// Guarded together: a grant must decrement the budget and record both
    /// windows atomically, or two concurrent requests could each observe a
    /// budget large enough and both spend it.
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    limiter: Limiter,
    spent_today: u64,
    day_started: SystemTime,
}

impl Faucet {
    /// Builds a faucet.
    ///
    /// # Errors
    ///
    /// [`FaucetError::ValueBearingChain`] if `chain_id` is one where value is
    /// real. Refused at construction rather than per request, so a
    /// misconfigured deployment fails to start instead of failing on the first
    /// request — which is the one somebody is watching.
    pub fn new(
        chain_id: impl Into<String>,
        dispense: u64,
        daily_cap: u64,
        now: SystemTime,
    ) -> Result<Self, FaucetError> {
        let chain_id = chain_id.into();
        if VALUE_BEARING_CHAINS.contains(&chain_id.as_str()) {
            return Err(FaucetError::ValueBearingChain(format!(
                "'{chain_id}' is value-bearing; a faucet holds a funded key in a web \
                 service and must not run there. See docs/mainnet-readiness.md."
            )));
        }

        Ok(Self {
            chain_id,
            dispense,
            daily_cap,
            state: Mutex::new(State {
                limiter: Limiter::new(),
                spent_today: 0,
                day_started: now,
            }),
        })
    }

    /// The chain this faucet serves.
    #[must_use]
    pub fn chain_id(&self) -> &str {
        &self.chain_id
    }

    /// Units handed out per grant.
    #[must_use]
    pub const fn dispense(&self) -> u64 {
        self.dispense
    }

    /// The day's ceiling, regardless of who asks.
    #[must_use]
    pub const fn daily_cap(&self) -> u64 {
        self.daily_cap
    }

    /// Attempts a grant.
    ///
    /// # Order of checks
    ///
    /// Address shape, then rate limits, then budget. Shape first because it is
    /// free and a malformed address should not consume anything. Budget
    /// **last** because a caller who is rate-limited should be told that — it
    /// is actionable — rather than being told the budget is gone, which is not.
    ///
    /// # Errors
    ///
    /// [`FaucetError::BadAddress`], [`FaucetError::RateLimited`], or
    /// [`FaucetError::BudgetExhausted`].
    pub fn request(
        &self,
        ip: IpAddr,
        address_hex: &str,
        now: SystemTime,
    ) -> Result<Grant, FaucetError> {
        let address = parse_address(address_hex)?;

        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            // A poisoned lock means a thread panicked mid-grant. Continuing is
            // right: the alternative is a faucet that stops limiting, and the
            // map is structurally intact either way.
            Err(poisoned) => poisoned.into_inner(),
        };

        // Roll the budget over before reading it, so a request arriving after
        // midnight sees a fresh day rather than yesterday's exhaustion.
        if now
            .duration_since(state.day_started)
            .is_ok_and(|elapsed| elapsed >= Duration::from_secs(24 * 60 * 60))
        {
            state.spent_today = 0;
            state.day_started = now;
        }

        state
            .limiter
            .check_and_commit(ip, &address, now)
            .map_err(FaucetError::RateLimited)?;

        let Some(spent) = state.spent_today.checked_add(self.dispense) else {
            return Err(FaucetError::BudgetExhausted);
        };
        if spent > self.daily_cap {
            return Err(FaucetError::BudgetExhausted);
        }
        state.spent_today = spent;

        Ok(Grant {
            amount: self.dispense,
            remaining_today: self.daily_cap - spent,
            next_request_in: limit::WINDOW.as_secs(),
        })
    }

    /// Units already handed out today.
    #[must_use]
    pub fn spent_today(&self) -> u64 {
        match self.state.lock() {
            Ok(guard) => guard.spent_today,
            Err(poisoned) => poisoned.into_inner().spent_today,
        }
    }
}

/// Parses a 32-byte hex address, with or without a `0x` prefix.
pub(crate) fn parse_address(input: &str) -> Result<[u8; 32], FaucetError> {
    let trimmed = input.strip_prefix("0x").unwrap_or(input);
    if trimmed.len() != 64 {
        return Err(FaucetError::BadAddress(format!(
            "got {} characters",
            trimmed.len()
        )));
    }
    let mut out = [0u8; 32];
    hex::decode_to_slice(trimmed, &mut out)
        .map_err(|_| FaucetError::BadAddress("not hex".to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600)
    }

    fn faucet() -> Faucet {
        Faucet::new("maya-genesis-rc1", 100, 1_000, now()).expect("a testnet chain")
    }

    fn addr(byte: u8) -> String {
        hex::encode([byte; 32])
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    #[test]
    fn a_faucet_refuses_to_exist_on_a_value_bearing_chain() {
        // Refused at construction, so a misconfigured deployment fails to
        // start rather than on the first request somebody is watching.
        for chain in ["mainnet", "maya-mainnet"] {
            let error = Faucet::new(chain, 100, 1_000, now()).expect_err("must refuse");
            assert!(matches!(error, FaucetError::ValueBearingChain(_)));
        }
    }

    #[test]
    fn a_grant_reports_what_is_left() {
        let faucet = faucet();
        let grant = faucet.request(ip(1), &addr(1), now()).expect("granted");
        assert_eq!(grant.amount, 100);
        assert_eq!(grant.remaining_today, 900);
        assert_eq!(grant.next_request_in, limit::WINDOW.as_secs());
    }

    #[test]
    fn the_daily_cap_bounds_the_loss_regardless_of_who_asks() {
        // The control that matters against a distributed request flood, which
        // no per-key limiter can see. Ten grants of 100 exhaust a cap of 1,000.
        let faucet = faucet();
        for byte in 0..10u8 {
            faucet
                .request(ip(byte), &addr(byte), now())
                .unwrap_or_else(|e| panic!("grant {byte} refused: {e}"));
        }

        let error = faucet
            .request(ip(200), &addr(200), now())
            .expect_err("the cap is reached");
        assert!(matches!(error, FaucetError::BudgetExhausted));
        assert_eq!(faucet.spent_today(), 1_000);
    }

    #[test]
    fn the_budget_rolls_over_after_a_day() {
        let faucet = faucet();
        for byte in 0..10u8 {
            faucet.request(ip(byte), &addr(byte), now()).expect("grant");
        }
        assert!(faucet.request(ip(200), &addr(200), now()).is_err());

        let tomorrow = now() + Duration::from_secs(24 * 60 * 60);
        faucet
            .request(ip(200), &addr(200), tomorrow)
            .expect("a new day has a new budget");
    }

    #[test]
    fn a_rate_limit_is_reported_before_the_budget() {
        // A rate-limited caller can act on that; "the budget is gone" is not
        // actionable and would be the wrong thing to tell them.
        let faucet = Faucet::new("maya-genesis-rc1", 100, 100, now()).expect("faucet");
        faucet.request(ip(1), &addr(1), now()).expect("first");

        let error = faucet
            .request(ip(1), &addr(2), now())
            .expect_err("both limits apply");
        assert!(
            matches!(error, FaucetError::RateLimited(_)),
            "the rate limit must be reported, not the exhausted budget: {error}"
        );
    }

    #[test]
    fn a_malformed_address_consumes_nothing() {
        // Shape is checked first, so a typo does not spend the caller's day.
        let faucet = faucet();
        for bad in ["", "0x", "zz", &"a".repeat(63), &"a".repeat(65)] {
            assert!(matches!(
                faucet.request(ip(1), bad, now()),
                Err(FaucetError::BadAddress(_))
            ));
        }
        faucet
            .request(ip(1), &addr(1), now())
            .expect("a refused malformed request must not have consumed the IP");
    }

    #[test]
    fn an_address_may_carry_a_prefix() {
        let faucet = faucet();
        faucet
            .request(ip(1), &format!("0x{}", addr(1)), now())
            .expect("prefixed");

        // The same address without the prefix is the same address.
        let error = faucet
            .request(ip(2), &addr(1), now())
            .expect_err("same address");
        assert!(matches!(
            error,
            FaucetError::RateLimited(Refusal::Address { .. })
        ));
    }

    #[test]
    fn one_ip_with_many_fresh_addresses_gets_one_grant() {
        // The headline property, at the service level rather than the
        // limiter's. Free keypair generation must not be free money.
        let faucet = Faucet::new("maya-genesis-rc1", 100, 1_000_000, now()).expect("faucet");
        faucet.request(ip(1), &addr(0), now()).expect("first");

        for byte in 1..=255u8 {
            assert!(
                faucet.request(ip(1), &addr(byte), now()).is_err(),
                "address {byte} from the same IP must be refused"
            );
        }
        assert_eq!(faucet.spent_today(), 100, "exactly one grant");
    }
}
