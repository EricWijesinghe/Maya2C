//! `pacs.008` — FI to FI Customer Credit Transfer.
//!
//! A bank telling another bank to pay a customer. It is the message a
//! cross-border retail payment actually travels in, and the one the bridge
//! spends most of its time reading.
//!
//! ## The shape, reduced to what this bridge acts on
//!
//! ```text
//! Document
//!   FIToFICstmrCdtTrf
//!     GrpHdr                       MsgId, CreDtTm, NbOfTxs
//!     CdtTrfTxInf            (1..n)
//!       PmtId                      InstrId?, EndToEndId
//!       IntrBkSttlmAmt Ccy=…       the amount that settles
//!       Dbtr / DbtrAcct / DbtrAgt
//!       Cdtr / CdtrAcct / CdtrAgt
//!       RmtInf?                    Ustrd?
//! ```
//!
//! Everything else the schema allows — charge bearer, regulatory reporting,
//! clearing system references, settlement method — is **not read and not
//! preserved**. A counterparty that needs one of those round-tripped is a
//! counterparty this bridge is not ready for, and saying so here is cheaper
//! than a field that silently disappears.
//!
//! ## `NbOfTxs` is checked, not trusted
//!
//! The group header declares how many transactions follow. A message whose
//! declaration disagrees with its contents is refused rather than reconciled to
//! either number: the two readings settle different amounts of money, and there
//! is no way to tell which one the sender meant.

use crate::amount::Amount;
use crate::error::{Error, Result};
use crate::party::{AccountId, Bic, Currency, Iban, Party};
use crate::xml::{self, Element};

/// The namespace this crate reads and writes.
pub const NAMESPACE: &str = "urn:iso:std:iso:20022:tech:xsd:pacs.008.001.08";

/// The most transactions one message may carry.
///
/// Below [`xml::MAX_CHILDREN`] so this bound is the one that fires, and its
/// error names the transaction count rather than a generic child limit.
pub const MAX_TRANSACTIONS: usize = 1_024;

/// The longest identifier field. `Max35Text`, per the schema.
pub const MAX_ID_CHARS: usize = 35;

/// The longest unstructured remittance line. `Max140Text`.
pub const MAX_REMITTANCE_CHARS: usize = 140;

/// One credit transfer within a message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreditTransfer {
    /// `PmtId/InstrId`, the sending bank's own reference.
    pub instruction_id: Option<String>,
    /// `PmtId/EndToEndId`, the reference that survives the whole chain.
    pub end_to_end_id: String,
    /// `IntrBkSttlmAmt`, in base units.
    pub amount: Amount,
    /// The amount's currency.
    pub currency: Currency,
    /// Debtor, account and agent.
    pub debtor: Party,
    /// Creditor, account and agent.
    pub creditor: Party,
    /// `RmtInf/Ustrd`, if present.
    pub remittance: Option<String>,
}

/// A whole `pacs.008` message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomerCreditTransfer {
    /// `GrpHdr/MsgId`.
    pub message_id: String,
    /// `GrpHdr/CreDtTm`, carried verbatim. See [`parse`].
    pub created_at: String,
    /// The transfers, in document order.
    pub transactions: Vec<CreditTransfer>,
}

/// Parses a `pacs.008` document.
///
/// `CreDtTm` is carried as the string it arrived as rather than parsed into a
/// timestamp. Nothing in this bridge decides anything by it — freshness on this
/// chain is measured in block height and never in a timestamp (invariant 9) —
/// and parsing it would mean choosing a calendar library whose leap-second and
/// offset handling becomes part of what a message means.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] for a document that is not a `pacs.008`,
/// [`Error::Missing`] for a mandatory element, [`Error::Bound`] for a message
/// past [`MAX_TRANSACTIONS`] or a field past its schema limit, and whatever
/// [`Amount::parse`] and the [`crate::party`] types return for a malformed
/// value.
pub fn parse(bytes: &[u8]) -> Result<CustomerCreditTransfer> {
    let document = xml::parse(bytes)?;
    if document.name != "Document" {
        return Err(Error::Unsupported(format!(
            "root element is <{}>, not <Document>",
            document.name
        )));
    }
    let body = document
        .child("FIToFICstmrCdtTrf")
        .ok_or_else(|| Error::Unsupported("not a pacs.008: no <FIToFICstmrCdtTrf>".to_owned()))?;
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
    // Declared versus actual. Refused rather than reconciled: the two readings
    // settle different amounts and nothing here can tell which the sender meant.
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

    Ok(CustomerCreditTransfer {
        message_id,
        created_at,
        transactions,
    })
}

/// One `CdtTrfTxInf`.
fn transaction(node: &Element) -> Result<CreditTransfer> {
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
    let amount = Amount::parse(&settlement.text)?;

    let remittance = node
        .child("RmtInf")
        .and_then(|info| info.text_of("Ustrd"))
        .map(|text| bounded(text, "Ustrd", MAX_REMITTANCE_CHARS))
        .transpose()?;

    Ok(CreditTransfer {
        instruction_id,
        end_to_end_id,
        amount,
        currency,
        debtor: party(node, "Dbtr", "DbtrAcct", "DbtrAgt")?,
        creditor: party(node, "Cdtr", "CdtrAcct", "CdtrAgt")?,
        remittance,
    })
}

/// A party from its three sibling elements.
///
/// Shared by `pacs.008` and `pacs.009`, which name the elements identically and
/// differ only in whether the party is a customer or an institution.
pub(crate) fn party(
    node: &Element,
    name_element: &'static str,
    account_element: &'static str,
    agent_element: &'static str,
) -> Result<Party> {
    let account = node.require(account_element)?.require("Id")?;
    let account = match (account.child("IBAN"), account.child("Othr")) {
        // Both forms at once is a message with two account numbers, and picking
        // one would be this crate deciding where the money goes.
        (Some(_), Some(_)) => {
            return Err(Error::Unexpected {
                element: account_element.to_owned(),
                reason: "carries both an IBAN and an Othr identifier",
            });
        }
        (Some(iban), None) => AccountId::Iban(Iban::parse(&iban.text)?),
        (None, Some(other)) => AccountId::other(other.require_text("Id")?)?,
        (None, None) => return Err(Error::Missing("IBAN or Othr/Id")),
    };

    let mut party = Party::new(account);
    if let Some(name) = node
        .child(name_element)
        .and_then(|party| party.text_of("Nm"))
    {
        party = party.with_name(name)?;
    }
    if let Some(bic) = node
        .child(agent_element)
        .and_then(|agent| agent.child("FinInstnId"))
        .and_then(|id| id.text_of("BICFI"))
    {
        party = party.with_agent(Bic::parse(bic)?);
    }
    Ok(party)
}

/// A field within its schema length, or [`Error::Bound`].
pub(crate) fn bounded(text: &str, what: &'static str, limit: usize) -> Result<String> {
    let length = text.chars().count();
    if length > limit {
        return Err(Error::Bound {
            what,
            found: length,
            limit,
        });
    }
    if text.is_empty() {
        return Err(Error::Invalid {
            field: what,
            reason: "is empty".to_owned(),
        });
    }
    Ok(text.to_owned())
}

impl CustomerCreditTransfer {
    /// Renders the message back to XML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Xml`] if the writer fails.
    pub fn to_xml(&self) -> Result<String> {
        let mut body = Element::new("FIToFICstmrCdtTrf").with(
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

impl CreditTransfer {
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
        // Party, account and agent are three *siblings* in the schema, not a
        // nested group, so they are appended rather than composed.
        element
            .children
            .extend(party_elements(&self.debtor, "Dbtr", "DbtrAcct", "DbtrAgt"));
        element.children.extend(party_elements(
            &self.creditor,
            "Cdtr",
            "CdtrAcct",
            "CdtrAgt",
        ));
        element.maybe(
            self.remittance
                .as_ref()
                .map(|text| Element::new("RmtInf").with(Element::leaf("Ustrd", text))),
        )
    }
}

/// The elements a party renders to, in the document order the schema fixes:
/// the party itself, then its account, then its agent.
///
/// One function returning several elements rather than a nested group, because
/// the schema puts them side by side — a wrapper element here would render an
/// `<Dbtr>` containing an `<DbtrAcct>`, which is a different document.
pub(crate) fn party_elements(
    party: &Party,
    name_element: &str,
    account_element: &str,
    agent_element: &str,
) -> Vec<Element> {
    let mut elements = Vec::with_capacity(3);

    let mut named = Element::new(name_element);
    if let Some(name) = &party.name {
        named = named.with(Element::leaf("Nm", name));
    }
    elements.push(named);

    let id = match &party.account {
        AccountId::Iban(iban) => Element::new("Id").with(Element::leaf("IBAN", iban.as_str())),
        AccountId::Other(other) => {
            Element::new("Id").with(Element::new("Othr").with(Element::leaf("Id", other.as_str())))
        }
    };
    elements.push(Element::new(account_element).with(id));

    if let Some(agent) = &party.agent {
        elements.push(
            Element::new(agent_element)
                .with(Element::new("FinInstnId").with(Element::leaf("BICFI", agent.as_str()))),
        );
    }
    elements
}
