//! Safe-by-default contract execution rules (Master Prompt 23 §1-2).
//!
//! The runtime, not the contract author, enforces:
//!
//! - **Resources move, never copy or vanish.** Balances of a resource type
//!   change only through [`Runtime::transfer`], [`Runtime::mint`] and
//!   [`Runtime::burn`]. There is no API that duplicates a balance, and minting
//!   or burning needs a capability for that type.
//! - **Capabilities.** A contract touches another holder's resources only
//!   through a [`Capability`] that holder granted, bounded by an amount.
//! - **Re-entrancy is off.** Entering a contract already on the call stack
//!   fails unless that function opted in.
//! - **Declared invariants.** A contract registers predicates over state.
//!   They are checked when each top-level call ends, and a violation reverts
//!   everything the call did.
//! - **Outflow rate limits.** A contract may cap how much leaves it per
//!   window. A transfer that would exceed the cap is queued for its guardians
//!   instead of executing.
//!
//! This is the rule layer the VM host calls: `crates/vm`'s opt-in
//! `maya_res` import module (`maya_vm::resources`) routes every value-moving
//! import through it. The consensus import surface (`env`) does not include
//! it; `reports/23-contract-safety.md` has what that means.

use std::collections::BTreeMap;

/// A contract or account identifier.
pub type Id = u32;
/// A resource type (a token, an NFT class).
pub type Kind = u32;

/// What a capability permits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Right {
    /// Move the grantor's resources of `kind`, up to the allowance.
    Move,
    /// Create new units of `kind`.
    Mint,
    /// Destroy units of `kind`.
    Burn,
}

/// A grant from `grantor` to `holder`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    /// Who granted it.
    pub grantor: Id,
    /// Who may use it.
    pub holder: Id,
    /// Resource type.
    pub kind: Kind,
    /// What it permits.
    pub right: Right,
    /// Remaining allowance.
    pub remaining: u128,
}

/// Why an operation failed. Any failure inside a call reverts the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// No capability covers the operation.
    NoCapability,
    /// Balance too low.
    Insufficient,
    /// Arithmetic would overflow: a resource cannot be conjured by wrapping.
    Overflow,
    /// A contract was re-entered without opting in.
    Reentered(Id),
    /// A declared invariant failed at the end of the call.
    Invariant(&'static str),
    /// The outflow exceeded the contract's rate limit; queued for guardians.
    RateLimited {
        /// Queue position.
        pending: usize,
    },
    /// The call body failed for a reason outside these rules — a VM trap or
    /// running out of gas. It reverts like any other fault.
    Aborted,
}

/// An invariant: a name and a predicate over the runtime's state.
pub type InvariantFn = fn(&State) -> bool;

/// The state invariants can read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Balance of `(holder, kind)`.
    pub balances: BTreeMap<(Id, Kind), u128>,
    /// Total supply per kind.
    pub supply: BTreeMap<Kind, u128>,
    /// Contract-defined counters (e.g. "total deposits").
    pub counters: BTreeMap<(Id, &'static str), u128>,
}

impl State {
    /// Balance of `holder` in `kind`.
    #[must_use]
    pub fn balance(&self, holder: Id, kind: Kind) -> u128 {
        self.balances.get(&(holder, kind)).copied().unwrap_or(0)
    }

    /// A contract counter.
    #[must_use]
    pub fn counter(&self, contract: Id, name: &'static str) -> u128 {
        self.counters.get(&(contract, name)).copied().unwrap_or(0)
    }
}

/// A per-contract outflow limit: at most `max` units of `kind` per `window` blocks.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    /// Resource type limited.
    pub kind: Kind,
    /// Units per window.
    pub max: u128,
    /// Window length, blocks.
    pub window: u64,
}

/// A queued outflow awaiting guardians.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pending {
    /// From.
    pub from: Id,
    /// To.
    pub to: Id,
    /// Kind.
    pub kind: Kind,
    /// Amount.
    pub amount: u128,
}

/// The runtime.
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    /// Current state.
    pub state: State,
    caps: Vec<Capability>,
    reentrant: BTreeMap<(Id, &'static str), bool>,
    invariants: BTreeMap<Id, Vec<(&'static str, InvariantFn)>>,
    limits: BTreeMap<Id, RateLimit>,
    outflow: BTreeMap<Id, (u64, u128)>,
    /// Outflows held for guardian review.
    pub pending: Vec<Pending>,
    stack: Vec<Id>,
    /// Current block height, for rate-limit windows.
    pub height: u64,
}

impl Runtime {
    /// Grants a capability.
    pub fn grant(&mut self, cap: Capability) {
        self.caps.push(cap);
    }

    /// Declares that `contract::function` may be re-entered.
    pub fn allow_reentry(&mut self, contract: Id, function: &'static str) {
        self.reentrant.insert((contract, function), true);
    }

    /// Declares an invariant for `contract`.
    pub fn declare_invariant(&mut self, contract: Id, name: &'static str, f: InvariantFn) {
        self.invariants.entry(contract).or_default().push((name, f));
    }

    /// Sets an outflow rate limit on `contract`.
    pub fn limit_outflow(&mut self, contract: Id, limit: RateLimit) {
        self.limits.insert(contract, limit);
    }

    fn use_cap(
        &mut self,
        grantor: Id,
        holder: Id,
        kind: Kind,
        right: Right,
        amount: u128,
    ) -> Result<(), Fault> {
        let cap = self
            .caps
            .iter_mut()
            .find(|c| {
                c.grantor == grantor
                    && c.holder == holder
                    && c.kind == kind
                    && c.right == right
                    && c.remaining >= amount
            })
            .ok_or(Fault::NoCapability)?;
        cap.remaining -= amount;
        Ok(())
    }

    fn check_rate(&mut self, from: Id, to: Id, kind: Kind, amount: u128) -> Result<(), Fault> {
        let Some(limit) = self.limits.get(&from).copied() else {
            return Ok(());
        };
        if limit.kind != kind {
            return Ok(());
        }
        let window = self.height / limit.window.max(1);
        let used = match self.outflow.get(&from) {
            Some((w, u)) if *w == window => *u,
            _ => 0,
        };
        if used.saturating_add(amount) > limit.max {
            self.pending.push(Pending {
                from,
                to,
                kind,
                amount,
            });
            return Err(Fault::RateLimited {
                pending: self.pending.len() - 1,
            });
        }
        self.outflow.insert(from, (window, used + amount));
        Ok(())
    }

    /// Moves `amount` of `kind` from `from` to `to`, on behalf of `actor`:
    /// the owner itself, or a holder of a Move capability from `from`.
    ///
    /// # Errors
    ///
    /// [`Fault`] for a missing capability, a short balance, an overflow, or a
    /// rate limit.
    pub fn transfer(
        &mut self,
        actor: Id,
        from: Id,
        to: Id,
        kind: Kind,
        amount: u128,
    ) -> Result<(), Fault> {
        if actor != from {
            self.use_cap(from, actor, kind, Right::Move, amount)?;
        }
        let have = self.state.balance(from, kind);
        if have < amount {
            return Err(Fault::Insufficient);
        }
        let dest = self
            .state
            .balance(to, kind)
            .checked_add(amount)
            .ok_or(Fault::Overflow)?;
        self.check_rate(from, to, kind, amount)?;
        self.state.balances.insert((from, kind), have - amount);
        self.state.balances.insert((to, kind), dest);
        Ok(())
    }

    /// Creates `amount` of `kind` for `to`, by a holder of the Mint capability.
    ///
    /// # Errors
    ///
    /// [`Fault::NoCapability`] or [`Fault::Overflow`].
    pub fn mint(&mut self, actor: Id, kind: Kind, to: Id, amount: u128) -> Result<(), Fault> {
        self.use_cap(0, actor, kind, Right::Mint, amount)?;
        let s = self
            .state
            .supply
            .get(&kind)
            .copied()
            .unwrap_or(0)
            .checked_add(amount)
            .ok_or(Fault::Overflow)?;
        let b = self
            .state
            .balance(to, kind)
            .checked_add(amount)
            .ok_or(Fault::Overflow)?;
        self.state.supply.insert(kind, s);
        self.state.balances.insert((to, kind), b);
        Ok(())
    }

    /// Destroys `amount` of `kind` held by `from`, by a holder of the Burn capability.
    ///
    /// # Errors
    ///
    /// [`Fault::NoCapability`] or [`Fault::Insufficient`].
    pub fn burn(&mut self, actor: Id, kind: Kind, from: Id, amount: u128) -> Result<(), Fault> {
        self.use_cap(0, actor, kind, Right::Burn, amount)?;
        let have = self.state.balance(from, kind);
        if have < amount {
            return Err(Fault::Insufficient);
        }
        self.state.balances.insert((from, kind), have - amount);
        let s = self.state.supply.get(&kind).copied().unwrap_or(0) - amount;
        self.state.supply.insert(kind, s);
        Ok(())
    }

    /// Sets a contract counter (a contract's own bookkeeping).
    pub fn set_counter(&mut self, contract: Id, name: &'static str, value: u128) {
        self.state.counters.insert((contract, name), value);
    }

    /// Runs `body` as a call into `contract::function`. Re-entry is refused
    /// unless opted in. At the end of the outermost call, every declared
    /// invariant is checked. Any fault or violated invariant restores the
    /// state as it was before the outermost call.
    ///
    /// # Errors
    ///
    /// The first [`Fault`] raised.
    pub fn call(
        &mut self,
        contract: Id,
        function: &'static str,
        body: impl FnOnce(&mut Self) -> Result<(), Fault>,
    ) -> Result<(), Fault> {
        if self.stack.contains(&contract)
            && !self
                .reentrant
                .get(&(contract, function))
                .copied()
                .unwrap_or(false)
        {
            return Err(Fault::Reentered(contract));
        }
        let outermost = self.stack.is_empty();
        let snapshot = outermost.then(|| {
            (
                self.state.clone(),
                self.caps.clone(),
                self.outflow.clone(),
                self.pending.len(),
            )
        });
        self.stack.push(contract);
        let mut result = body(self);
        self.stack.pop();
        if outermost && result.is_ok() {
            result = self.check_invariants();
        }
        if let (Err(e), Some((state, caps, outflow, queued))) = (&result, snapshot) {
            // An outflow this call queued survives its revert — that is the
            // point of queuing; anything else this call added is dropped.
            if !matches!(e, Fault::RateLimited { .. }) {
                self.pending.truncate(queued);
            }
            self.state = state;
            self.caps = caps;
            self.outflow = outflow;
        }
        result
    }

    fn check_invariants(&self) -> Result<(), Fault> {
        for list in self.invariants.values() {
            for (name, f) in list {
                if !f(&self.state) {
                    return Err(Fault::Invariant(name));
                }
            }
        }
        Ok(())
    }
}
