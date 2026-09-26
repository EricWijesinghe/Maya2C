//! Clear signing: what you see is what you sign (Master Prompt 22 §4).
//!
//! Three pieces, all pure functions over plain data:
//!
//! - [`Effect`] — the transaction intent standard. An app *claims* the effects
//!   of a transaction as a list of these; the wallet *simulates* the
//!   transaction and gets its own list.
//! - [`render`] — each effect in plain language, the same words on the phone,
//!   the explorer and the hardware wallet.
//! - [`review`] — compares claim and simulation and checks the simulated
//!   effects against the patterns drainers use, producing warnings the wallet
//!   must show before the user signs.
//!
//! The review never trusts the claim: every warning is computed from the
//! **simulated** effects, and a claim that differs from the simulation is
//! itself the loudest warning. That is the Bybit lesson — the interface
//! showed one transaction and the signers signed another.

use std::collections::BTreeSet;

/// A 32-byte address.
pub type Address = [u8; 32];

/// An amount at or above this is treated as "unlimited" (2^96).
pub const UNLIMITED: u128 = 1 << 96;
/// Outflow of at least this share of a balance, in percent, is a drain.
pub const DRAIN_PERCENT: u128 = 90;

/// One effect of a transaction.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effect {
    /// Value leaves the account.
    Transfer {
        /// Token (all-zero = the native coin).
        token: Address,
        /// Recipient.
        to: Address,
        /// Amount in base units.
        amount: u128,
    },
    /// Another party may spend up to `amount` of `token` later.
    Approve {
        /// Token.
        token: Address,
        /// Who may spend.
        spender: Address,
        /// Allowance.
        amount: u128,
    },
    /// Another party may move every item of a collection.
    ApproveAll {
        /// Collection.
        collection: Address,
        /// Operator granted.
        operator: Address,
    },
    /// An off-chain signed allowance (permit-style), usable without a
    /// transaction from the owner.
    Permit {
        /// Token.
        token: Address,
        /// Who may spend.
        spender: Address,
        /// Allowance.
        amount: u128,
    },
    /// A contract's code is replaced.
    Upgrade {
        /// Contract.
        contract: Address,
    },
    /// A contract's owner or an account's key changes.
    SetOwner {
        /// Contract or account.
        target: Address,
        /// New owner.
        owner: Address,
    },
    /// A call with no value effect the simulator could attribute.
    Call {
        /// Contract.
        contract: Address,
        /// Method name, as declared in the contract's metadata.
        method: String,
    },
}

/// What the wallet knows about the user and the world.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// Addresses the user has sent to before or saved.
    pub contacts: BTreeSet<Address>,
    /// Contracts whose source is verified and whose reputation is known.
    pub verified: BTreeSet<Address>,
    /// The user's balance per token before the transaction.
    pub balances: Vec<(Address, u128)>,
}

/// A warning the wallet must show. Ordered by severity, highest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Warning {
    /// The simulation shows effects the app did not claim, or amounts differ.
    ClaimMismatch,
    /// A contract's code or an owner/key changes.
    ControlChange,
    /// An allowance with no practical limit.
    UnlimitedApproval,
    /// Every item of a collection handed to an operator.
    ApproveAll,
    /// An off-chain permit signature: can drain later with no further prompt.
    PermitSignature,
    /// A recipient that looks like a contact but is not one.
    LookAlikeRecipient,
    /// Nearly all of a token balance leaves at once.
    DrainsBalance,
    /// An allowance to a spender that is not a verified contract.
    UnverifiedSpender,
    /// A call to a contract that is not verified.
    UnverifiedContract,
}

fn hex4(a: &Address) -> String {
    format!("{:02x}{:02x}…{:02x}{:02x}", a[0], a[1], a[30], a[31])
}

/// The effect in plain English. Amounts are base units; the wallet applies
/// the token's decimals where a token declares them.
#[must_use]
pub fn render(e: &Effect) -> String {
    match e {
        Effect::Transfer { token, to, amount } => {
            format!("Send {amount} of token {} to {}", hex4(token), hex4(to))
        }
        Effect::Approve {
            token,
            spender,
            amount,
        } if *amount >= UNLIMITED => {
            format!(
                "Let {} spend ALL of your token {}, now and later",
                hex4(spender),
                hex4(token)
            )
        }
        Effect::Approve {
            token,
            spender,
            amount,
        } => format!(
            "Let {} spend up to {amount} of your token {}",
            hex4(spender),
            hex4(token)
        ),
        Effect::ApproveAll {
            collection,
            operator,
        } => format!(
            "Let {} move EVERY item in collection {}",
            hex4(operator),
            hex4(collection)
        ),
        Effect::Permit {
            token,
            spender,
            amount,
        } => {
            format!(
                "Sign a permit letting {} spend up to {amount} of token {} without asking you again",
                hex4(spender),
                hex4(token)
            )
        }
        Effect::Upgrade { contract } => format!("Replace the code of contract {}", hex4(contract)),
        Effect::SetOwner { target, owner } => {
            format!("Make {} the owner of {}", hex4(owner), hex4(target))
        }
        Effect::Call { contract, method } => {
            format!("Call `{method}` on contract {}", hex4(contract))
        }
    }
}

/// Differs from a contact but shares its first two and last two bytes: the
/// shape of an address-poisoning look-alike.
fn looks_like_contact(to: &Address, ctx: &Context) -> bool {
    !ctx.contacts.contains(to)
        && ctx
            .contacts
            .iter()
            .any(|c| c[..2] == to[..2] && c[30..] == to[30..])
}

fn warnings_for(e: &Effect, ctx: &Context, out: &mut BTreeSet<Warning>) {
    match e {
        Effect::Transfer { token, to, amount } => {
            if looks_like_contact(to, ctx) {
                out.insert(Warning::LookAlikeRecipient);
            }
            let balance = ctx
                .balances
                .iter()
                .find(|(t, _)| t == token)
                .map_or(0, |(_, b)| *b);
            if balance > 0 && amount.saturating_mul(100) >= balance.saturating_mul(DRAIN_PERCENT) {
                out.insert(Warning::DrainsBalance);
            }
        }
        Effect::Approve {
            spender, amount, ..
        } => {
            if *amount >= UNLIMITED {
                out.insert(Warning::UnlimitedApproval);
            }
            if !ctx.verified.contains(spender) {
                out.insert(Warning::UnverifiedSpender);
            }
        }
        Effect::ApproveAll { .. } => {
            out.insert(Warning::ApproveAll);
        }
        Effect::Permit {
            spender, amount, ..
        } => {
            out.insert(Warning::PermitSignature);
            if *amount >= UNLIMITED {
                out.insert(Warning::UnlimitedApproval);
            }
            if !ctx.verified.contains(spender) {
                out.insert(Warning::UnverifiedSpender);
            }
        }
        Effect::Upgrade { .. } | Effect::SetOwner { .. } => {
            out.insert(Warning::ControlChange);
        }
        Effect::Call { contract, .. } => {
            if !ctx.verified.contains(contract) {
                out.insert(Warning::UnverifiedContract);
            }
        }
    }
}

/// Reviews a transaction before signing: the app's `claimed` effects against
/// the wallet's `simulated` ones, and the simulated effects against known
/// drainer patterns. Returns warnings, most severe first.
#[must_use]
pub fn review(claimed: &[Effect], simulated: &[Effect], ctx: &Context) -> Vec<Warning> {
    let mut out = BTreeSet::new();
    let (c, s): (BTreeSet<&Effect>, BTreeSet<&Effect>) =
        (claimed.iter().collect(), simulated.iter().collect());
    if c != s {
        out.insert(Warning::ClaimMismatch);
    }
    for e in simulated {
        warnings_for(e, ctx, &mut out);
    }
    out.into_iter().collect()
}
