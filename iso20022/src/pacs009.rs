//! `pacs.009` — Financial Institution Credit Transfer.
//!
//! A bank moving its own money to another bank, rather than a customer's. The
//! wire format is nearly the same as [`pacs.008`](crate::pacs008) — the
//! difference that matters to a bridge is *whose* money moves, and that is a
//! difference in what the message means rather than in how it is parsed.
//!
//! ## Why this is a separate type and not a flag
//!
//! The two messages settle against different accounts and reconcile into
//! different statement lines. A single type with an `is_institution: bool`
//! would put that distinction in a field that every downstream match has to
//! remember to read, and the one that forgot would credit a customer from a
//! nostro account. Two types make the compiler ask.
//!
//! ```text
//! Document
//!   FinInstnCdtTrf
//!     GrpHdr                       MsgId, CreDtTm, NbOfTxs
//!     CdtTrfTxInf            (1..n)
//!       PmtId                      InstrId?, EndToEndId
//!       IntrBkSttlmAmt Ccy=…
//!       Dbtr / DbtrAcct / DbtrAgt  the *institutions*
//!       Cdtr / CdtrAcct / CdtrAgt
//! ```
//!
//! There is no `RmtInf`: a bank-to-bank transfer carries no remittance advice
//! for a customer to reconcile against, and a parser that accepted one would be
//! accepting a field the schema does not have.

use crate::amount::Amount;
use crate::error::{Error, Result};
use crate::pacs008::{MAX_ID_CHARS, MAX_TRANSACTIONS, bounded, party, party_elements};
use crate::party::{Currency, Party};
use crate::xml::{self, Element};

/// The namespace this crate reads and writes.
pub const NAMESPACE: &str = "urn:iso:std:iso:20022:tech:xsd:pacs.009.001.08";

/// One institution-to-institution transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstitutionTransfer {
    /// `PmtId/InstrId`.
    pub instruction_id: Option<String>,
    /// `PmtId/EndToEndId`.
    pub end_to_end_id: String,
    /// `IntrBkSttlmAmt`, in base units.
    pub amount: Amount,
    /// The amount's currency.
    pub currency: Currency,
    /// The debtor institution.
    pub debtor: Party,
    /// The creditor institution.
    pub creditor: Party,
}

/// A whole `pacs.009` message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinancialInstitutionTransfer {
    /// `GrpHdr/MsgId`.
    pub message_id: String,
    /// `GrpHdr/CreDtTm`, verbatim. See [`crate::pacs008::parse`] for why.
    pub created_at: String,
    /// The transfers, in document order.
    pub transactions: Vec<InstitutionTransfer>,
}

/// Parses a `pacs.009` document.
///
/// # Errors
///
/// The same set as [`crate::pacs008::parse`], and [`Error::Unexpected`] for a
/// message carrying `RmtInf`, which this message type does not have.
pub fn parse(bytes: &[u8]) -> Result<FinancialInstitutionTransfer> {
    let document = xml::parse(bytes)?;
    if document.name != "Document" {
        return Err(Error::Unsupported(format!(
            "root element is <{}>, not <Document>",
            document.name
        )));
    }
    let body = document
        .child("FinInstnCdtTrf")
        .ok_or_else(|| Error::Unsupported("not a pacs.009: no <FinInstnCdtTrf>".to_owned()))?;
    let header = body.require("GrpHdr")?;

    let message_id = bounded(header.require_text("MsgId")?, "MsgId", MAX_ID_CHARS)?;
    let created_at = bounded(header.require_text("CreDtTm")?, "CreDtTm", MAX_ID_CHARS)?;

    let transactions = body
        .all("CdtTrfTxInf")
        .map(transaction)
        .collect::<Result<Vec<_>>>()?;

    if transactions.is_empty() {
        return Err(Error::Missing("CdtTrfTxInf"));
    }
    if transactions.len() > MAX_TRANSACTIONS {
        return Err(Error::Bound {
            what: "transactions in a message",
            found: transactions.len(),
            limit: MAX_TRANSACTIONS,
        });
    }
    if let Some(declared) = header.text_of("NbOfTxs") {
        let declared = declared.parse::<usize>().map_err(|_| Error::Invalid {
            field: "NbOfTxs",
            reason: format!("{declared:?} is not a count"),
        })?;
        if declared != transactions.len() {
            return Err(Error::Invalid {
                field: "NbOfTxs",
                reason: format!(
                    "declares {declared} transactions, carries {}",
                    transactions.len()
                ),
            });
        }
    }

    Ok(FinancialInstitutionTransfer {
        message_id,
        created_at,
        transactions,
    })
}

/// One `CdtTrfTxInf`.
fn transaction(node: &Element) -> Result<InstitutionTransfer> {
    if node.child("RmtInf").is_some() {
        return Err(Error::Unexpected {
            element: "RmtInf".to_owned(),
            reason: "a pacs.009 carries no remittance information",
        });
    }

    let payment_id = node.require("PmtId")?;
    let instruction_id = payment_id
        .text_of("InstrId")
        .map(|text| bounded(text, "InstrId", MAX_ID_CHARS))
        .transpose()?;
    let end_to_end_id = bounded(
        payment_id.require_text("EndToEndId")?,
        "EndToEndId",
        MAX_ID_CHARS,
    )?;

    let settlement = node.require("IntrBkSttlmAmt")?;
    let currency = Currency::parse(settlement.attribute("Ccy").ok_or(Error::Invalid {
        field: "IntrBkSttlmAmt",
        reason: "no Ccy attribute".to_owned(),
    })?)?;

    Ok(InstitutionTransfer {
        instruction_id,
        end_to_end_id,
        amount: Amount::parse(&settlement.text)?,
        currency,
        debtor: party(node, "Dbtr", "DbtrAcct", "DbtrAgt")?,
        creditor: party(node, "Cdtr", "CdtrAcct", "CdtrAgt")?,
    })
}

impl FinancialInstitutionTransfer {
    /// Renders the message back to XML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Xml`] if the writer fails.
    pub fn to_xml(&self) -> Result<String> {
        let mut body = Element::new("FinInstnCdtTrf").with(
            Element::new("GrpHdr")
                .with(Element::leaf("MsgId", &self.message_id))
                .with(Element::leaf("CreDtTm", &self.created_at))
                .with(Element::leaf(
                    "NbOfTxs",
                    self.transactions.len().to_string(),
                )),
        );
        for transaction in &self.transactions {
            body = body.with(transaction.to_element());
        }
        xml::render(&Element::new("Document").with(body), NAMESPACE)
    }
}

impl InstitutionTransfer {
    /// The `CdtTrfTxInf` element for this transfer.
    fn to_element(&self) -> Element {
        let mut element = Element::new("CdtTrfTxInf")
            .with(
                Element::new("PmtId")
                    .maybe(
                        self.instruction_id
                            .as_ref()
                            .map(|id| Element::leaf("InstrId", id)),
                    )
                    .with(Element::leaf("EndToEndId", &self.end_to_end_id)),
            )
            .with(
                Element::leaf("IntrBkSttlmAmt", self.amount.to_iso_decimal())
                    .attr("Ccy", self.currency.as_str()),
            );
        element
            .children
            .extend(party_elements(&self.debtor, "Dbtr", "DbtrAcct", "DbtrAgt"));
        element.children.extend(party_elements(
            &self.creditor,
            "Cdtr",
            "CdtrAcct",
            "CdtrAgt",
        ));
        element
    }
}
