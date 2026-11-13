//! Multisig commands (P-16): set up an m-of-n account, propose a spend,
//! approve it behind the same native confirm screen as a transfer, merge the
//! other members' approvals, finish.
//!
//! The account and the proposal travel as hex text the person copies between
//! members; nothing is stored. Every judgement (policy rules, approval
//! checks, the confirm text) is the wallet core's.

use maya_wallet_core::confirm::{Confirmation, approval_message};
use maya_wallet_core::multisig::{
    MemberKey, MultisigAccount, Proposal, ProposalStatus as ProposalView,
};
use maya_wallet_core::networks;
use maya_wallet_core::payment::SignedTransfer;
use maya_wallet_core::wallet::SignTerms;
use serde::Serialize;

use crate::commands::{
    DialogSlot, FeeTerms, Session, native_ok, parse_amount_arg, parse_genesis, priced_fee_terms,
};

/// An account as the multisig screen shows it.
#[derive(Serialize)]
pub struct AccountView {
    /// The policy, hex: what every member pastes to open the same account.
    pub account_hex: String,
    /// The account's address, hex.
    pub address: String,
    /// The address in grouped Bech32m for `genesis`'s network.
    pub grouped: String,
    /// `m`.
    pub threshold: usize,
    /// The members, in policy order.
    pub members: Vec<MemberKey>,
}

fn account_view(account: &MultisigAccount, genesis: &str) -> AccountView {
    let network = networks::address_network(genesis);
    AccountView {
        account_hex: account.to_hex(),
        address: hex::encode(account.address()),
        grouped: maya_clear_sign::grouped(network, &account.address()),
        threshold: account.threshold(),
        members: account.members(),
    }
}

fn proposal_view(proposal: &Proposal) -> ProposalView {
    proposal.status()
}

/// Account `index`'s member key, to give to whoever sets the account up.
#[tauri::command]
pub fn multisig_member_key(
    index: u32,
    session: tauri::State<'_, Session>,
) -> Result<MemberKey, String> {
    session.with(|w| w.member_key(index).map_err(|e| e.to_string()))
}

/// Builds a `threshold`-of-`members.len()` account.
#[tauri::command]
pub fn multisig_create(
    threshold: usize,
    members: Vec<MemberKey>,
    genesis: String,
) -> Result<AccountView, String> {
    let account = MultisigAccount::new(threshold, &members).map_err(|e| e.to_string())?;
    Ok(account_view(&account, &genesis))
}

/// Opens an account another member shared.
#[tauri::command]
pub fn multisig_open(account_hex: String, genesis: String) -> Result<AccountView, String> {
    let account = MultisigAccount::from_hex(&account_hex).map_err(|e| e.to_string())?;
    Ok(account_view(&account, &genesis))
}

/// The fee presets for a spend from the account, priced for the finished v8
/// frame (the policy and `m` approvals), not for a single-signature transfer.
#[tauri::command]
pub async fn multisig_fee_options(
    node_url: String,
    account_hex: String,
) -> Result<FeeTerms, String> {
    let account = MultisigAccount::from_hex(&account_hex).map_err(|e| e.to_string())?;
    priced_fee_terms(&node_url, |_, _| {
        account.spend_size().map_err(|e| e.to_string())
    })
    .await
}

/// An unapproved spend from the account; `nonce` is the multisig account's,
/// `base_fee` the node's per-byte rate ("0" where fees are off).
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps each JS argument to one parameter.
pub fn multisig_propose(
    index: u32,
    account_hex: String,
    recipient: String,
    amount: String,
    fee: String,
    fee_to: Option<String>,
    nonce: u64,
    genesis: String,
    base_fee: String,
    session: tauri::State<'_, Session>,
) -> Result<ProposalView, String> {
    let base_fee = parse_amount_arg("base fee", &base_fee)?;
    let account = MultisigAccount::from_hex(&account_hex).map_err(|e| e.to_string())?;
    let chain = parse_genesis(&genesis)?;
    let amount = parse_amount_arg("amount", &amount)?;
    let terms = SignTerms {
        fee: parse_amount_arg("fee", &fee)?,
        fee_to: fee_to.as_deref(),
        nonce,
        chain: &chain,
        allow_high_fee: false,
    };
    session.with(|w| {
        w.propose_multisig(index, &account, &recipient, amount, &terms, base_fee)
            .map(|p| proposal_view(&p))
            .map_err(|e| e.to_string())
    })
}

/// How far the proposal in `proposal_hex` has got, read from that text alone,
/// so the screen never shows another proposal's progress.
#[tauri::command]
pub fn multisig_status(proposal_hex: String, genesis: String) -> Result<ProposalView, String> {
    let chain = parse_genesis(&genesis)?;
    Proposal::from_hex(&proposal_hex, &chain)
        .map(|p| proposal_view(&p))
        .map_err(|e| e.to_string())
}

/// The confirm screen's content for approving a proposal. Signs nothing.
#[tauri::command]
pub fn multisig_preview(
    app: tauri::AppHandle,
    proposal_hex: String,
    genesis: String,
    session: tauri::State<'_, Session>,
) -> Result<Confirmation, String> {
    let chain = parse_genesis(&genesis)?;
    let proposal = Proposal::from_hex(&proposal_hex, &chain).map_err(|e| e.to_string())?;
    let ctx = session.review_context(&app)?;
    session.with(|w| Ok(w.confirm_multisig(&proposal, &chain, &ctx)))
}

/// Adds account `index`'s approval, after the native confirm screen.
/// `confirmed_hash` must be the hash [`multisig_preview`] returned;
/// `base_fee` is the node's per-byte rate ("0" where fees are off).
#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps each JS argument to one parameter.
pub async fn multisig_approve(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    index: u32,
    proposal_hex: String,
    genesis: String,
    confirmed_hash: String,
    base_fee: String,
    session: tauri::State<'_, Session>,
) -> Result<ProposalView, String> {
    let _slot = DialogSlot::take(session.dialog_open())?;
    let chain = parse_genesis(&genesis)?;
    let base_fee = parse_amount_arg("base fee", &base_fee)?;
    let proposal = Proposal::from_hex(&proposal_hex, &chain).map_err(|e| e.to_string())?;
    let ctx = session.review_context(&app)?;
    let shown = session.with(|w| Ok(w.confirm_multisig(&proposal, &chain, &ctx)))?;
    if shown.signing_hash != confirmed_hash {
        return Err(
            "the proposal differs from the one on the confirm screen; nothing was signed"
                .to_string(),
        );
    }
    let message = approval_message(&shown);
    if !native_ok(&app, &window, "Approve multisig spend", message, "Approve").await? {
        return Err("cancelled on the native confirm screen; nothing was signed".to_string());
    }
    session.with(|w| {
        w.approve_multisig(index, &proposal, &chain, &shown.signing_hash, base_fee)
            .map(|p| proposal_view(&p))
            .map_err(|e| e.to_string())
    })
}

/// Combines two members' copies of the same spend.
#[tauri::command]
pub fn multisig_merge(
    proposal_hex: String,
    other_hex: String,
    genesis: String,
) -> Result<ProposalView, String> {
    let chain = parse_genesis(&genesis)?;
    let read = |text: &str| Proposal::from_hex(text, &chain).map_err(|e| e.to_string());
    let merged = read(&proposal_hex)?
        .merge(&read(&other_hex)?, &chain)
        .map_err(|e| e.to_string())?;
    Ok(proposal_view(&merged))
}

/// The broadcastable spend, once the quorum verifies.
#[tauri::command]
pub fn multisig_finish(proposal_hex: String, genesis: String) -> Result<SignedTransfer, String> {
    let chain = parse_genesis(&genesis)?;
    Proposal::from_hex(&proposal_hex, &chain)
        .and_then(|p| p.finish(&chain))
        .map_err(|e| e.to_string())
}
