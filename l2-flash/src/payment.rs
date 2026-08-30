//! Assembling an atomic multi-hop payment from a route.
//!
//! Every hop carries an HTLC locked to the **same** hash. That single shared
//! lock is what makes the payment atomic: the only way for the final recipient
//! to claim is to reveal the preimage, and once revealed, every upstream hop can
//! claim with it too. There is no state in which the recipient is paid but an
//! intermediary is not.
//!
//! ## The expiry ladder
//!
//! Expiries decrease along the route, by [`HOP_EXPIRY_DELTA`] per hop. An
//! intermediary's incoming HTLC must outlive its outgoing one, or it could be
//! claimed downstream after its own claim window upstream had already shut —
//! paying out with no way to be paid. The ladder is what removes that exposure.

use custom_l1_node::core::ChannelId;

use crate::error::{FlashError, Result};
use crate::htlc::{HOP_EXPIRY_DELTA, HashLock, HopHtlc, Preimage, hash_lock};
use crate::routing::{NodeId, Route};

/// The HTLC to install on one hop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payment {
    /// Channel the HTLC belongs to.
    pub channel_id: ChannelId,
    /// Node offering the HTLC.
    pub from: NodeId,
    /// Node receiving it.
    pub to: NodeId,
    /// Value locked on this hop.
    pub amount: u64,
    /// Shared hash lock.
    pub hash_lock: HashLock,
    /// Expiry for this hop.
    pub expiry_height: u64,
}

/// A fully specified multi-hop payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaymentPlan {
    /// Hops in send order.
    pub hops: Vec<Payment>,
    /// Hash every hop is locked to.
    pub hash_lock: HashLock,
    /// Value the sender commits.
    pub total_amount: u64,
    /// Value the recipient receives.
    pub delivered: u64,
    /// Expiry of the first hop, the longest in the ladder.
    pub max_expiry: u64,
}

impl PaymentPlan {
    /// Builds a plan from a route and a preimage held by the recipient.
    ///
    /// `current_height` anchors the expiry ladder; `final_cltv` is the margin
    /// the recipient keeps to claim after everything upstream is committed.
    ///
    /// # Errors
    ///
    /// Returns [`FlashError::NoRoute`] for an empty route, or
    /// [`FlashError::BalanceOverflow`] if the expiry ladder overflows.
    pub fn build(
        route: &Route,
        preimage: &Preimage,
        current_height: u64,
        final_cltv: u64,
    ) -> Result<Self> {
        if route.hops.is_empty() {
            return Err(FlashError::NoRoute {
                from: String::from("<empty route>"),
                to: String::from("<empty route>"),
                amount: route.delivered,
            });
        }

        let lock = hash_lock(preimage);

        // The last hop expires soonest. Build the ladder from the tail up so
        // each hop strictly outlives the one after it.
        let hop_count = route.hops.len() as u64;
        let final_expiry = current_height
            .checked_add(final_cltv)
            .ok_or(FlashError::BalanceOverflow)?;
        let max_expiry = final_expiry
            .checked_add(
                HOP_EXPIRY_DELTA
                    .checked_mul(hop_count.saturating_sub(1))
                    .ok_or(FlashError::BalanceOverflow)?,
            )
            .ok_or(FlashError::BalanceOverflow)?;

        let mut hops = Vec::with_capacity(route.hops.len());
        for (index, hop) in route.hops.iter().enumerate() {
            let steps_remaining = route.hops.len() - 1 - index;
            let expiry_height = final_expiry
                .checked_add(HOP_EXPIRY_DELTA * steps_remaining as u64)
                .ok_or(FlashError::BalanceOverflow)?;

            hops.push(Payment {
                channel_id: hop.channel_id,
                from: hop.from,
                to: hop.to,
                amount: hop.amount_in,
                hash_lock: lock,
                expiry_height,
            });
        }

        Ok(Self {
            hops,
            hash_lock: lock,
            total_amount: route.total_amount,
            delivered: route.delivered,
            max_expiry,
        })
    }

    /// Checks that expiries strictly decrease along the route.
    ///
    /// A violation means some intermediary is exposed, so this is a hard
    /// invariant rather than a preference.
    #[must_use]
    pub fn expiries_are_laddered(&self) -> bool {
        self.hops
            .windows(2)
            .all(|pair| pair[0].expiry_height > pair[1].expiry_height)
    }

    /// Checks that forwarded amounts never increase along the route.
    ///
    /// An intermediary must never be asked to forward more than it received.
    #[must_use]
    pub fn amounts_are_monotonic(&self) -> bool {
        self.hops
            .windows(2)
            .all(|pair| pair[0].amount >= pair[1].amount)
    }

    /// The HTLC parameters for each hop.
    #[must_use]
    pub fn hop_htlcs(&self) -> Vec<HopHtlc> {
        self.hops
            .iter()
            .map(|hop| HopHtlc {
                channel_id: hop.channel_id,
                amount: hop.amount,
                hash_lock: hop.hash_lock,
                expiry_height: hop.expiry_height,
            })
            .collect()
    }
}
