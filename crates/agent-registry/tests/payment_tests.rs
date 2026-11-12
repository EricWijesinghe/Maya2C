mod common;

use common::{NOW, signer};
use maya_agent_registry::card::AgentId;
use maya_agent_registry::error::Error;
use maya_agent_registry::payment::{
    PaymentBook, Quote, SignedQuote, Transfer, draft_payment, issue_receipt, verify_receipt,
};
use primitive_types::U256;

fn quote_for(seller: &AgentId, buyer: &AgentId) -> Quote {
    Quote {
        quote_id: [7; 32],
        payee: *seller,
        payer: *buyer,
        capability: "translate".into(),
        amount: U256::from(5u64),
        payee_address: [3; 32],
        expires_at: NOW + 600,
    }
}

/// The transfer the payer would make from the draft: same to, amount and memo.
fn paying(draft: &maya_agent_registry::payment::PaymentDraft, txid: u8) -> Transfer {
    Transfer {
        txid: [txid; 32],
        to: draft.to,
        amount: draft.amount,
        memo: draft.memo,
        height: 10,
    }
}

#[test]
fn a_quote_is_paid_once() {
    let seller = signer(1);
    let buyer = signer(2);
    let quote = SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller)
        .expect("seller signs own quote");
    let draft = draft_payment(&quote, seller.public_key(), &buyer.agent_id(), NOW).expect("draft");
    let mut book = PaymentBook::default();

    assert!(issue_receipt(&mut book, &seller, &quote, &paying(&draft, 9), NOW).is_ok());
    // A second transfer carrying the same memo must not earn a second receipt.
    let replay = issue_receipt(&mut book, &seller, &quote, &paying(&draft, 8), NOW);
    assert_eq!(replay, Err(Error::Replay));
}

#[test]
fn a_receipt_verifies_only_against_the_transfer_it_names() {
    let seller = signer(1);
    let buyer = signer(2);
    let quote =
        SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller).expect("sign");
    let draft = draft_payment(&quote, seller.public_key(), &buyer.agent_id(), NOW).expect("draft");
    let transfer = paying(&draft, 9);
    let mut book = PaymentBook::default();
    let receipt = issue_receipt(&mut book, &seller, &quote, &transfer, NOW).expect("receipt");

    assert_eq!(
        verify_receipt(&receipt, seller.public_key(), &transfer),
        Ok(())
    );

    let mut other_txid = transfer.clone();
    other_txid.txid = [1; 32];
    assert_eq!(
        verify_receipt(&receipt, seller.public_key(), &other_txid),
        Err(Error::Mismatch)
    );

    let mut other_memo = transfer.clone();
    other_memo.memo = [1; 32];
    assert_eq!(
        verify_receipt(&receipt, seller.public_key(), &other_memo),
        Err(Error::Mismatch)
    );

    let mut other_amount = transfer.clone();
    other_amount.amount = U256::from(6u64);
    assert_eq!(
        verify_receipt(&receipt, seller.public_key(), &other_amount),
        Err(Error::Mismatch)
    );
}

#[test]
fn a_transfer_with_the_wrong_memo_does_not_spend_the_quote() {
    let seller = signer(1);
    let buyer = signer(2);
    let quote =
        SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller).expect("sign");
    let draft = draft_payment(&quote, seller.public_key(), &buyer.agent_id(), NOW).expect("draft");
    let mut book = PaymentBook::default();

    let mut wrong = paying(&draft, 9);
    wrong.memo = [0; 32];
    assert_eq!(
        issue_receipt(&mut book, &seller, &quote, &wrong, NOW),
        Err(Error::Mismatch)
    );
    // The quote is still open for the transfer that does bind it.
    assert!(issue_receipt(&mut book, &seller, &quote, &paying(&draft, 9), NOW).is_ok());
}

#[test]
fn a_tampered_receipt_is_refused() {
    let seller = signer(1);
    let buyer = signer(2);
    let quote =
        SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller).expect("sign");
    let draft = draft_payment(&quote, seller.public_key(), &buyer.agent_id(), NOW).expect("draft");
    let transfer = paying(&draft, 9);
    let mut book = PaymentBook::default();
    let mut receipt = issue_receipt(&mut book, &seller, &quote, &transfer, NOW).expect("receipt");

    receipt.receipt.amount = U256::from(999u64);
    assert_eq!(
        verify_receipt(&receipt, seller.public_key(), &transfer),
        Err(Error::Signature)
    );
}

#[test]
fn an_expired_quote_is_refused_before_payment() {
    let seller = signer(1);
    let buyer = signer(2);
    let quote = quote_for(&seller.agent_id(), &buyer.agent_id());
    let expires = quote.expires_at;
    let signed = SignedQuote::sign(quote, &seller).expect("sign");
    let result = draft_payment(&signed, seller.public_key(), &buyer.agent_id(), expires);
    assert_eq!(result, Err(Error::Expired));
}

#[test]
fn a_quote_addressed_to_another_payer_is_refused() {
    let seller = signer(1);
    let buyer = signer(2);
    let carol = signer(3);
    let signed =
        SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller).expect("sign");
    let result = draft_payment(&signed, seller.public_key(), &carol.agent_id(), NOW);
    assert_eq!(result, Err(Error::NotPayer));
}

#[test]
fn a_tampered_quote_is_refused() {
    let seller = signer(1);
    let buyer = signer(2);
    let mut signed =
        SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller).expect("sign");
    signed.quote.amount = U256::from(1u64);
    let result = draft_payment(&signed, seller.public_key(), &buyer.agent_id(), NOW);
    assert_eq!(result, Err(Error::Signature));
}

#[test]
fn only_the_payee_can_issue_a_receipt() {
    let seller = signer(1);
    let buyer = signer(2);
    let quote =
        SignedQuote::sign(quote_for(&seller.agent_id(), &buyer.agent_id()), &seller).expect("sign");
    let draft = draft_payment(&quote, seller.public_key(), &buyer.agent_id(), NOW).expect("draft");
    let mut book = PaymentBook::default();
    let result = issue_receipt(&mut book, &buyer, &quote, &paying(&draft, 9), NOW);
    assert_eq!(result, Err(Error::NotPayee));
}
