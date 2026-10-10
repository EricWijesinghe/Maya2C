//! `camt.053` — Bank to Customer Statement.
//!
//! The outbound direction, and the one that makes this a bridge rather than an
//! importer. A statement is what a bank's counterparty reconciles against: an
//! opening balance, the entries that happened, and a closing balance that the
//! entries have to explain.
//!
//! ## The balances are checked against the entries
//!
//! `OPBD + Σ(credits − debits) == CLBD`, enforced by [`Statement::seal`] and
//! again by [`parse`]. A statement whose closing balance does not follow from
//! its own entries is not a statement — it is two numbers that will be
//! reconciled against different ledgers and disagree in a month.
//!
//! This is the same shape as the chain's own value conservation
//! (`src/state/invariant_guard/conservation.rs`): a total that must equal the
//! sum of what moved, checked rather than assumed, because the failure is
//! silent and expensive.
//!
//! ## Balances are unsigned, with a direction beside them
//!
//! ISO 20022 carries sign in `CdtDbtInd`, never in the amount, and this crate
//! follows it: [`Amount`] is a `u64` and cannot be negative. An account
//! overdrawn is `DBIT` with a positive amount, which is also why the running
//! total here is an `i128` internally and a `(direction, amount)` pair on the
//! wire — the intermediate can go negative even when neither endpoint does.
//!
//! ```text
//! Document
//!   BkToCstmrStmt
//!     GrpHdr          MsgId, CreDtTm
//!     Stmt
//!       Id, CreDtTm
//!       Acct/Id       IBAN or Othr
//!       Bal (2..n)    OPBD and CLBD, each Amt Ccy=… + CdtDbtInd
//!       Ntry (0..n)   Amt Ccy=…, CdtDbtInd, Sts, NtryRef?
//! ```

use crate::amount::Amount;
use crate::error::{Error, Result};
use crate::pacs008::{MAX_ID_CHARS, bounded};
use crate::party::{AccountId, Currency, Iban};
use crate::xml::{self, Element};

/// The namespace this crate reads and writes.
pub const NAMESPACE: &str = "urn:iso:std:iso:20022:tech:xsd:camt.053.001.08";

/// The most entries one statement may carry.
pub const MAX_ENTRIES: usize = 2_048;

/// Which way the money went.
///
/// ISO 20022 spells these `CRDT` and `DBIT`, and they are the only two values
/// `CdtDbtInd` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Money in. `CRDT`.
    Credit,
    /// Money out. `DBIT`.
    Debit,
}

impl Direction {
    /// The wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Credit => "CRDT",
            Self::Debit => "DBIT",
        }
    }

    /// The direction a code names.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for anything but `CRDT` or `DBIT`. There is
    /// no default: guessing a direction is guessing which way the money went.
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "CRDT" => Ok(Self::Credit),
            "DBIT" => Ok(Self::Debit),
            _ => Err(Error::Invalid {
                field: "CdtDbtInd",
                reason: format!("{text:?} is neither CRDT nor DBIT"),
            }),
        }
    }

    /// The amount as a signed contribution to a running balance.
    const fn signed(self, amount: u64) -> i128 {
        match self {
            Self::Credit => amount as i128,
            Self::Debit => -(amount as i128),
        }
    }
}

/// One line of a statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// `NtryRef`, the reference this entry reconciles by. On the `Maya2C` side
    /// this carries the transaction id the entry was drawn from.
    pub reference: Option<String>,
    /// `Amt`, in base units.
    pub amount: Amount,
    /// `CdtDbtInd`.
    pub direction: Direction,
}

/// A statement for one account over one period.
///
/// Construct with [`Statement::seal`], which is the only way to get one whose
/// balances agree with its entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Statement {
    /// `GrpHdr/MsgId`.
    pub message_id: String,
    /// `Stmt/Id`.
    pub statement_id: String,
    /// `CreDtTm`, verbatim.
    pub created_at: String,
    /// The account this statement is for.
    pub account: AccountId,
    /// The currency every balance and entry is in.
    pub currency: Currency,
    /// `OPBD`, the balance before the first entry.
    pub opening: Amount,
    /// `CLBD`, the balance after the last.
    pub closing: Amount,
    /// The entries, in order.
    pub entries: Vec<Entry>,
}

impl Statement {
    /// Builds a statement and checks that its entries explain its closing
    /// balance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Bound`] past [`MAX_ENTRIES`], and [`Error::Invalid`] if
    /// `opening + Σ entries != closing`, or if any intermediate or final
    /// balance is negative — an account cannot hold a negative amount in a
    /// format whose amounts are unsigned, and rendering one would mean choosing
    /// a sign convention ISO 20022 does not have.
    pub fn seal(
        message_id: impl Into<String>,
        statement_id: impl Into<String>,
        created_at: impl Into<String>,
        account: AccountId,
        currency: Currency,
        opening: Amount,
        entries: Vec<Entry>,
    ) -> Result<Self> {
        if entries.len() > MAX_ENTRIES {
            return Err(Error::Bound {
                what: "statement entries",
                found: entries.len(),
                limit: MAX_ENTRIES,
            });
        }
        let closing = closing_balance(opening, &entries)?;
        Ok(Self {
            message_id: message_id.into(),
            statement_id: statement_id.into(),
            created_at: created_at.into(),
            account,
            currency,
            opening,
            closing,
            entries,
        })
    }

    /// Renders the statement to XML.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Xml`] if the writer fails.
    pub fn to_xml(&self) -> Result<String> {
        let account_id = match &self.account {
            AccountId::Iban(iban) => Element::new("Id").with(Element::leaf("IBAN", iban.as_str())),
            AccountId::Other(other) => {
                Element::new("Id").with(Element::new("Othr").with(Element::leaf("Id", other)))
            }
        };

        let mut statement = Element::new("Stmt")
            .with(Element::leaf("Id", &self.statement_id))
            .with(Element::leaf("CreDtTm", &self.created_at))
            .with(Element::new("Acct").with(account_id))
            .with(self.balance("OPBD", self.opening))
            .with(self.balance("CLBD", self.closing));

        for entry in &self.entries {
            statement = statement.with(
                Element::new("Ntry")
                    .with(
                        Element::leaf("Amt", entry.amount.to_iso_decimal())
                            .attr("Ccy", self.currency.as_str()),
                    )
                    .with(Element::leaf("CdtDbtInd", entry.direction.as_str()))
                    // Every entry a statement renders is drawn from a committed
                    // block, so there is no pending state to report.
                    .with(Element::leaf("Sts", "BOOK"))
                    .maybe(
                        entry
                            .reference
                            .as_ref()
                            .map(|reference| Element::leaf("NtryRef", reference)),
                    ),
            );
        }

        let body = Element::new("BkToCstmrStmt")
            .with(
                Element::new("GrpHdr")
                    .with(Element::leaf("MsgId", &self.message_id))
                    .with(Element::leaf("CreDtTm", &self.created_at)),
            )
            .with(statement);
        xml::render(&Element::new("Document").with(body), NAMESPACE)
    }

    /// One `Bal` element.
    ///
    /// Always `CRDT`: both balances are checked non-negative by
    /// [`Statement::seal`], so there is no case where a `DBIT` balance is
    /// reachable, and emitting the indicator unconditionally keeps the reader
    /// and the writer symmetric.
    fn balance(&self, code: &str, amount: Amount) -> Element {
        Element::new("Bal")
            .with(
                Element::new("Tp").with(Element::new("CdOrPrtry").with(Element::leaf("Cd", code))),
            )
            .with(Element::leaf("Amt", amount.to_iso_decimal()).attr("Ccy", self.currency.as_str()))
            .with(Element::leaf("CdtDbtInd", Direction::Credit.as_str()))
    }
}

/// `opening + Σ entries`, refusing a total that leaves the unsigned range.
fn closing_balance(opening: Amount, entries: &[Entry]) -> Result<Amount> {
    // i128 for the running total: the intermediate can go negative on a debit
    // that a later credit covers, and `u64` would have to refuse a sequence
    // whose endpoints are both fine.
    let mut running = i128::from(opening.base_units());
    for entry in entries {
        running += entry.direction.signed(entry.amount.base_units());
    }
    if running < 0 {
        return Err(Error::Invalid {
            field: "CLBD",
            reason: format!("entries take the balance to {running}, and a balance is unsigned"),
        });
    }
    u64::try_from(running)
        .map(Amount::from_base_units)
        .map_err(|_| Error::Invalid {
            field: "CLBD",
            reason: format!("entries take the balance to {running}, past u64 base units"),
        })
}

/// Parses a `camt.053` document.
///
/// The balances are re-derived from the entries and checked against the ones
/// the document declares, so a statement that does not add up is refused here
/// exactly as it would be refused on the way out.
///
/// # Errors
///
/// Returns [`Error::Unsupported`] for a document that is not a `camt.053`,
/// [`Error::Missing`] for a mandatory element, [`Error::Invalid`] for a
/// declared closing balance the entries do not produce, and [`Error::Bound`]
/// past [`MAX_ENTRIES`].
pub fn parse(bytes: &[u8]) -> Result<Statement> {
    let document = xml::parse(bytes)?;
    if document.name != "Document" {
        return Err(Error::Unsupported(format!(
            "root element is <{}>, not <Document>",
            document.name
        )));
    }
    let body = document
        .child("BkToCstmrStmt")
        .ok_or_else(|| Error::Unsupported("not a camt.053: no <BkToCstmrStmt>".to_owned()))?;
    let header = body.require("GrpHdr")?;
    let statement = body.require("Stmt")?;

    let message_id = bounded(header.require_text("MsgId")?, "MsgId", MAX_ID_CHARS)?;
    let statement_id = bounded(statement.require_text("Id")?, "Id", MAX_ID_CHARS)?;
    let created_at = bounded(statement.require_text("CreDtTm")?, "CreDtTm", MAX_ID_CHARS)?;

    let account = statement.require("Acct")?.require("Id")?;
    let account = match (account.child("IBAN"), account.child("Othr")) {
        (Some(_), Some(_)) => {
            return Err(Error::Unexpected {
                element: "Acct".to_owned(),
                reason: "carries both an IBAN and an Othr identifier",
            });
        }
        (Some(iban), None) => AccountId::Iban(Iban::parse(&iban.text)?),
        (None, Some(other)) => AccountId::other(other.require_text("Id")?)?,
        (None, None) => return Err(Error::Missing("IBAN or Othr/Id")),
    };

    let (opening, currency) = balance(statement, "OPBD")?;
    let (declared_closing, closing_currency) = balance(statement, "CLBD")?;
    if closing_currency != currency {
        return Err(Error::Invalid {
            field: "Bal",
            reason: format!(
                "opening is {} and closing is {}",
                currency.as_str(),
                closing_currency.as_str()
            ),
        });
    }

    let entries = statement
        .all("Ntry")
        .map(|node| entry(node, currency))
        .collect::<Result<Vec<_>>>()?;
    if entries.len() > MAX_ENTRIES {
        return Err(Error::Bound {
            what: "statement entries",
            found: entries.len(),
            limit: MAX_ENTRIES,
        });
    }

    let closing = closing_balance(opening, &entries)?;
    if closing != declared_closing {
        return Err(Error::Invalid {
            field: "CLBD",
            reason: format!(
                "declares {} but its entries produce {}",
                declared_closing.to_iso_decimal(),
                closing.to_iso_decimal()
            ),
        });
    }

    Ok(Statement {
        message_id,
        statement_id,
        created_at,
        account,
        currency,
        opening,
        closing,
        entries,
    })
}

/// The `Bal` element with this type code, as an amount and a currency.
fn balance(statement: &Element, code: &'static str) -> Result<(Amount, Currency)> {
    let node = statement
        .all("Bal")
        .find(|balance| {
            balance
                .child("Tp")
                .and_then(|kind| kind.child("CdOrPrtry"))
                .and_then(|kind| kind.text_of("Cd"))
                == Some(code)
        })
        .ok_or(Error::Missing(code))?;
    let amount = node.require("Amt")?;
    let currency = Currency::parse(amount.attribute("Ccy").ok_or(Error::Invalid {
        field: "Bal/Amt",
        reason: "no Ccy attribute".to_owned(),
    })?)?;
    Ok((Amount::parse(&amount.text)?, currency))
}

/// One `Ntry`.
fn entry(node: &Element, expected: Currency) -> Result<Entry> {
    let amount = node.require("Amt")?;
    let currency = Currency::parse(amount.attribute("Ccy").ok_or(Error::Invalid {
        field: "Ntry/Amt",
        reason: "no Ccy attribute".to_owned(),
    })?)?;
    // A statement mixing currencies has no single balance to reconcile, and
    // summing across them would be inventing an exchange rate.
    if currency != expected {
        return Err(Error::Invalid {
            field: "Ntry/Amt",
            reason: format!(
                "entry is {} in a {} statement",
                currency.as_str(),
                expected.as_str()
            ),
        });
    }
    Ok(Entry {
        reference: node
            .text_of("NtryRef")
            .map(|text| bounded(text, "NtryRef", MAX_ID_CHARS))
            .transpose()?,
        amount: Amount::parse(&amount.text)?,
        direction: Direction::parse(node.require_text("CdtDbtInd")?)?,
    })
}
