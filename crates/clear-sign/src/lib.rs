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

pub use maya_address::Network;
use maya_amount::Amount;

/// A 32-byte address.
pub type Address = [u8; 32];

/// An amount at or above this is treated as "unlimited" (2^96).
pub const UNLIMITED: Amount = Amount([0, 1 << 32, 0, 0]);
/// Outflow of at least this share of a balance, in percent, is a drain.
pub const DRAIN_PERCENT: u64 = 90;
// The drain test below is written for 90% (`10a >= 9b`, `a >= b - b/10`).
const _: () = assert!(DRAIN_PERCENT == 90);

/// Characters per group when an address is shown in full (ADR-081 §6.1).
pub const GROUP_LEN: usize = 4;

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
        amount: Amount,
    },
    /// Another party may spend up to `amount` of `token` later.
    Approve {
        /// Token.
        token: Address,
        /// Who may spend.
        spender: Address,
        /// Allowance.
        amount: Amount,
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
        amount: Amount,
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
    pub balances: Vec<(Address, Amount)>,
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

/// One line per warning, in the words every confirm screen shows: here, so the
/// desktop wallet and the browser extension cannot word one differently.
#[must_use]
pub fn warning_text(w: Warning) -> &'static str {
    match w {
        Warning::ClaimMismatch => "What the app claims differs from what this transaction does.",
        Warning::ControlChange => "This changes who controls an account or a contract.",
        Warning::UnlimitedApproval => "This allows spending with no practical limit.",
        Warning::ApproveAll => "This hands every item of a collection to someone else.",
        Warning::PermitSignature => "This signature can be used to spend later without asking you.",
        Warning::LookAlikeRecipient => {
            "This recipient looks like one of your contacts but is a different address. Compare every group."
        }
        Warning::DrainsBalance => "This sends 90% or more of your balance.",
        Warning::UnverifiedSpender => "The spender is not a verified contract.",
        Warning::UnverifiedContract => "This calls a contract that is not verified.",
    }
}

/// An address in full Bech32m on `network`, the data part in groups of
/// [`GROUP_LEN`]: `maya1 q8f3 k2x…`. Display only — copy the ungrouped form.
#[must_use]
pub fn grouped(network: Network, a: &Address) -> String {
    let text = maya_address::encode(network, a);
    let data_at = network.hrp().len() + 1;
    let groups: Vec<String> = text.as_bytes()[data_at..]
        .chunks(GROUP_LEN)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    format!("{} {}", &text[..data_at], groups.join(" "))
}

/// The effect in plain English, every address in full Bech32m on `network`.
/// Amounts are base units; the wallet applies the token's decimals where a
/// token declares them.
#[must_use]
pub fn render(e: &Effect, network: Network) -> String {
    let full = |a: &Address| grouped(network, a);
    match e {
        // The all-zero token is the native coin; "token 0000…0000" reads as a
        // suspicious unknown token, which is the opposite of the truth.
        Effect::Transfer { token, to, amount } if *token == [0; 32] => {
            format!(
                "Send {amount} base units of the native coin to {}",
                full(to)
            )
        }
        Effect::Transfer { token, to, amount } => {
            format!("Send {amount} of token {} to {}", full(token), full(to))
        }
        Effect::Approve {
            token,
            spender,
            amount,
        } if *amount >= UNLIMITED => {
            format!(
                "Let {} spend ALL of your token {}, now and later",
                full(spender),
                full(token)
            )
        }
        Effect::Approve {
            token,
            spender,
            amount,
        } => format!(
            "Let {} spend up to {amount} of your token {}",
            full(spender),
            full(token)
        ),
        Effect::ApproveAll {
            collection,
            operator,
        } => format!(
            "Let {} move EVERY item in collection {}",
            full(operator),
            full(collection)
        ),
        Effect::Permit {
            token,
            spender,
            amount,
        } => {
            format!(
                "Sign a permit letting {} spend up to {amount} of token {} without asking you again",
                full(spender),
                full(token)
            )
        }
        Effect::Upgrade { contract } => format!("Replace the code of contract {}", full(contract)),
        Effect::SetOwner { target, owner } => {
            format!("Make {} the owner of {}", full(owner), full(target))
        }
        Effect::Call { contract, method } => {
            format!("Call `{method}` on contract {}", full(contract))
        }
    }
}

/// Differs from a contact but shares what a person checks: its first two and
/// last two bytes (the hex form), or the first and last group of its grouped
/// Bech32m form on any network (the checksum differs per prefix, so all are
/// checked).
fn looks_like_contact(to: &Address, ctx: &Context) -> bool {
    !ctx.contacts.contains(to)
        && ctx
            .contacts
            .iter()
            .any(|c| (c[..2] == to[..2] && c[30..] == to[30..]) || text_look_alike(c, to))
}

/// The first and the last group of the [`grouped`] display match: the
/// characters a person compares at a glance. The data part is 59 characters,
/// so that is 4 leading (the version symbol and 3 payload symbols) and 3
/// trailing; derived from the string, so it follows the display if either
/// changes. 30 varying bits per network: cheap to grind, which is why it must
/// warn.
#[must_use]
pub fn text_look_alike(a: &Address, b: &Address) -> bool {
    [Network::Mainnet, Network::Testnet, Network::Devnet]
        .into_iter()
        .any(|network| {
            let shown_a = maya_address::encode(network, a);
            let shown_b = maya_address::encode(network, b);
            let data = network.hrp().len() + 1;
            let tail_len = match (shown_a.len() - data) % GROUP_LEN {
                0 => GROUP_LEN,
                rest => rest,
            };
            let head = data..data + GROUP_LEN;
            let tail = shown_a.len() - tail_len..;
            shown_a[head.clone()] == shown_b[head] && shown_a[tail.clone()] == shown_b[tail]
        })
}

/// `amount` is at least 90% of a non-zero `balance`, decided within 256 bits:
/// `10a >= 9b` by checked additions, and where that would overflow,
/// `a >= b - b/10`, which is the same inequality for integers.
#[must_use]
pub fn drains(amount: Amount, balance: Amount) -> bool {
    if balance.is_zero() {
        return false;
    }
    match (times(amount, 10), times(balance, 9)) {
        (Some(a10), Some(b9)) => a10 >= b9,
        _ => amount >= balance.saturating_sub(div10(balance)),
    }
}

/// `a × k` by repeated checked addition, so no wider intermediate exists.
fn times(a: Amount, k: u8) -> Option<Amount> {
    (1..k).try_fold(a, |sum, _| sum.checked_add(a))
}

/// `a / 10`, rounded down, limb by limb (most significant first).
fn div10(a: Amount) -> Amount {
    let mut out = [0u64; maya_amount::LIMBS];
    let mut rem = 0u128;
    for i in (0..maya_amount::LIMBS).rev() {
        let cur = (rem << 64) | u128::from(a.0[i]);
        // Below 2^64 because rem < 10.
        out[i] = u64::try_from(cur / 10).unwrap_or(u64::MAX);
        rem = cur % 10;
    }
    Amount(out)
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
                .map_or(Amount::default(), |(_, b)| *b);
            if drains(*amount, balance) {
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
