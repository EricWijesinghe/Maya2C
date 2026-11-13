//! The multisig half of the IPC bridge (P-16). Same rule as `bridge.rs`:
//! nothing here can obtain key material, and every check is the core's.

use serde::{Deserialize, Serialize};

use super::{Confirmation, FeeTerms, SignedTransfer, call};

/// One member key: the suite byte and the public key, hex.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberKey {
    /// `0x30` hybrid, `0x10` ML-DSA-65, ...
    pub suite: u8,
    /// Public key, hex.
    pub public_key: String,
}

/// A multisig account as the core describes it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// The policy, hex: what every member pastes to open the account.
    pub account_hex: String,
    /// Address, hex.
    pub address: String,
    /// Address in grouped Bech32m.
    pub grouped: String,
    /// `m`.
    pub threshold: usize,
    /// Members in policy order.
    pub members: Vec<MemberKey>,
}

/// A proposal and its approvals so far.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Proposal {
    /// The transaction with its approvals, hex: what members pass on.
    pub proposal_hex: String,
    /// Positions of the members who approved.
    pub approved_by: Vec<u8>,
    /// `m`.
    pub threshold: usize,
    /// True once it can be finished.
    pub complete: bool,
}

/// Account `index`'s member key.
pub async fn member_key(index: u32) -> Result<MemberKey, String> {
    #[derive(Serialize)]
    struct Args {
        index: u32,
    }
    call("multisig_member_key", &Args { index }).await
}

/// Builds an account from `members`; the core checks every rule.
pub async fn create(
    threshold: usize,
    members: &[MemberKey],
    genesis: &str,
) -> Result<Account, String> {
    #[derive(Serialize)]
    struct Args<'a> {
        threshold: usize,
        members: &'a [MemberKey],
        genesis: &'a str,
    }
    call(
        "multisig_create",
        &Args {
            threshold,
            members,
            genesis,
        },
    )
    .await
}

/// Opens an account another member shared.
pub async fn open(account_hex: &str, genesis: &str) -> Result<Account, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        account_hex: &'a str,
        genesis: &'a str,
    }
    call(
        "multisig_open",
        &Args {
            account_hex,
            genesis,
        },
    )
    .await
}

/// An unapproved spend from the account; `nonce` is the account's own.
pub async fn propose(
    index: u32,
    account_hex: &str,
    recipient: &str,
    amount: &str,
    fee: &str,
    nonce: u64,
    terms: &FeeTerms,
) -> Result<Proposal, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        index: u32,
        account_hex: &'a str,
        recipient: &'a str,
        amount: &'a str,
        fee: &'a str,
        fee_to: Option<&'a str>,
        nonce: u64,
        genesis: &'a str,
        base_fee: &'a str,
    }
    call(
        "multisig_propose",
        &Args {
            index,
            account_hex,
            recipient,
            amount,
            fee,
            fee_to: terms.collector.as_deref(),
            nonce,
            genesis: &terms.genesis,
            base_fee: &terms.base_fee,
        },
    )
    .await
}

/// Fee presets priced for the finished multisig spend (policy and approvals).
pub async fn fee_options(node_url: &str, account_hex: &str) -> Result<FeeTerms, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        node_url: &'a str,
        account_hex: &'a str,
    }
    call(
        "multisig_fee_options",
        &Args {
            node_url,
            account_hex,
        },
    )
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProposalArgs<'a> {
    proposal_hex: &'a str,
    genesis: &'a str,
}

/// How far the proposal in `proposal_hex` has got, from that text alone.
pub async fn status(proposal_hex: &str, genesis: &str) -> Result<Proposal, String> {
    call(
        "multisig_status",
        &ProposalArgs {
            proposal_hex,
            genesis,
        },
    )
    .await
}

/// The confirm screen for approving a proposal. Signs nothing.
pub async fn preview(proposal_hex: &str, genesis: &str) -> Result<Confirmation, String> {
    call(
        "multisig_preview",
        &ProposalArgs {
            proposal_hex,
            genesis,
        },
    )
    .await
}

/// Approves as account `index`, after the native confirm screen.
pub async fn approve(
    index: u32,
    proposal_hex: &str,
    genesis: &str,
    confirmed_hash: &str,
    base_fee: &str,
) -> Result<Proposal, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        index: u32,
        proposal_hex: &'a str,
        genesis: &'a str,
        confirmed_hash: &'a str,
        base_fee: &'a str,
    }
    call(
        "multisig_approve",
        &Args {
            index,
            proposal_hex,
            genesis,
            confirmed_hash,
            base_fee,
        },
    )
    .await
}

/// Combines another member's copy of the same spend.
pub async fn merge(proposal_hex: &str, other_hex: &str, genesis: &str) -> Result<Proposal, String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Args<'a> {
        proposal_hex: &'a str,
        other_hex: &'a str,
        genesis: &'a str,
    }
    call(
        "multisig_merge",
        &Args {
            proposal_hex,
            other_hex,
            genesis,
        },
    )
    .await
}

/// The broadcastable spend, once the quorum verifies.
pub async fn finish(proposal_hex: &str, genesis: &str) -> Result<SignedTransfer, String> {
    call(
        "multisig_finish",
        &ProposalArgs {
            proposal_hex,
            genesis,
        },
    )
    .await
}
