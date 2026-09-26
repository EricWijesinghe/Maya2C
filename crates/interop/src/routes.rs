//! Per-route value caps that grow with incident-free time.
//!
//! A route (a connection to another chain) may move at most `cap(now)` per
//! window. The cap starts at `base` and grows linearly with clean days, up
//! to `ceiling`. An incident — a failed proof, a paused relayer, a detected
//! forgery — resets the clean clock to zero. So a bug in a young route can
//! lose at most its small starting cap, and a route earns trust only by
//! running without trouble.

/// A route's limits and history.
#[derive(Clone, Debug)]
pub struct Route {
    /// Cap per window on day zero.
    pub base: u128,
    /// Cap added per clean day.
    pub per_clean_day: u128,
    /// Highest the cap can grow.
    pub ceiling: u128,
    /// Window length, blocks.
    pub window: u64,
    /// Blocks per day, for the clean clock.
    pub blocks_per_day: u64,
    clean_since: u64,
    used: (u64, u128),
}

/// Why a transfer was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouteError {
    /// This window's cap is spent.
    OverCap {
        /// Cap at this height.
        cap: u128,
        /// Already moved this window.
        used: u128,
    },
}

impl Route {
    /// A route opened at `height`.
    #[must_use]
    pub fn new(
        base: u128,
        per_clean_day: u128,
        ceiling: u128,
        window: u64,
        blocks_per_day: u64,
        height: u64,
    ) -> Self {
        Self {
            base,
            per_clean_day,
            ceiling,
            window: window.max(1),
            blocks_per_day: blocks_per_day.max(1),
            clean_since: height,
            used: (u64::MAX, 0),
        }
    }

    /// The cap at `height`.
    #[must_use]
    pub fn cap(&self, height: u64) -> u128 {
        let days = u128::from(height.saturating_sub(self.clean_since) / self.blocks_per_day);
        self.base
            .saturating_add(self.per_clean_day.saturating_mul(days))
            .min(self.ceiling)
    }

    /// Moves `amount` across the route if the window's cap allows.
    ///
    /// # Errors
    ///
    /// [`RouteError::OverCap`] if it does not.
    pub fn send(&mut self, amount: u128, height: u64) -> Result<(), RouteError> {
        let window = height / self.window;
        let used = if self.used.0 == window {
            self.used.1
        } else {
            0
        };
        let cap = self.cap(height);
        if used.saturating_add(amount) > cap {
            return Err(RouteError::OverCap { cap, used });
        }
        self.used = (window, used + amount);
        Ok(())
    }

    /// Records an incident: the clean clock restarts, so the cap falls to `base`.
    pub fn incident(&mut self, height: u64) {
        self.clean_since = height;
    }
}
