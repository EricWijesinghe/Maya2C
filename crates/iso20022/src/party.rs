//! Who the money is coming from and going to.
//!
//! ## Why these are types and not strings
//!
//! An IBAN, a BIC and a proprietary account reference are all "a string" to a
//! parser and three different things to a bank. Keeping them apart in the type
//! system means the bridge cannot hand an IBAN where a BIC belongs, and means
//! validation happens exactly once — at the boundary, on the way in — rather
//! than at each of the places that later use the value.
//!
//! ## What is checked, and what is not
//!
//! [`Iban`] checks length, charset and the **mod-97 checksum**, because the
//! checksum is the whole point of the format: it is what turns a transposed
//! pair of digits into a rejected message instead of a payment to a stranger.
//! It does not check that the country code is a real country or that the
//! account exists — the first is a table that goes stale and the second is not
//! a question a parser can answer.
//!
//! [`Bic`] checks length and charset. It does not check the institution
//! register, for the same reason.
//!
//! Neither is an authorisation. A well-formed identifier is a well-formed
//! identifier; whether its owner may be paid is [`crate::sanctions`]' question.

use crate::error::{Error, Result};

/// The longest a proprietary account identifier may be.
///
/// ISO 20022 constrains `Othr/Id` to 34 characters, the same as an IBAN's
/// maximum. Enforced here rather than trusted, because it arrives from a
/// counterparty.
const MAX_ACCOUNT_ID_CHARS: usize = 34;

/// The longest a party name may be. `Max140Text`, per the schema.
pub const MAX_NAME_CHARS: usize = 140;

/// A Business Identifier Code: 8 or 11 characters, ISO 9362.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Bic(String);

impl Bic {
    /// Parses a BIC.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] unless the value is 8 or 11 characters of
    /// uppercase ASCII letters and digits. Lowercase is refused rather than
    /// upcased: a bridge that normalised silently would accept two spellings of
    /// one identifier and hash them differently everywhere downstream.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = |reason: &str| Error::Invalid {
            field: "BIC",
            reason: format!("{text:?}: {reason}"),
        };
        if text.len() != 8 && text.len() != 11 {
            return Err(invalid("a BIC is 8 or 11 characters"));
        }
        if !text
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(invalid("a BIC is uppercase letters and digits"));
        }
        Ok(Self(text.to_owned()))
    }

    /// The code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An International Bank Account Number, checksum verified.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Iban(String);

impl Iban {
    /// Parses and checksums an IBAN.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for a value outside 15..=34 characters, one
    /// that is not uppercase alphanumeric, one whose first two characters are
    /// not letters, or one whose mod-97 remainder is not 1.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = |reason: &str| Error::Invalid {
            field: "IBAN",
            reason: format!("{text:?}: {reason}"),
        };
        // The shortest national IBAN in use is 15 (Norway); the standard caps
        // the longest at 34. Checked before the checksum so a megabyte of
        // digits is a rejection rather than a long division.
        if !(15..=34).contains(&text.len()) {
            return Err(invalid("an IBAN is 15 to 34 characters"));
        }
        if !text
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return Err(invalid("an IBAN is uppercase letters and digits"));
        }
        if !text.as_bytes()[..2].iter().all(u8::is_ascii_uppercase) {
            return Err(invalid("an IBAN starts with a country code"));
        }
        if mod97(text) != 1 {
            return Err(invalid("checksum failed"));
        }
        Ok(Self(text.to_owned()))
    }

    /// The account number.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The IBAN check: move the first four characters to the end, letters become
/// two digits each, and the whole thing mod 97 must be 1.
///
/// Folded a digit at a time rather than built into one big integer, because the
/// number is up to 68 digits and this needs no bignum to be exact.
fn mod97(iban: &str) -> u32 {
    let bytes = iban.as_bytes();
    let mut remainder = 0u32;
    for index in 0..bytes.len() {
        let byte = bytes[(index + 4) % bytes.len()];
        remainder = if byte.is_ascii_digit() {
            remainder * 10 + u32::from(byte - b'0')
        } else {
            // 'A' is 10, 'Z' is 35 — two decimal digits, so the shift is 100.
            remainder * 100 + u32::from(byte - b'A') + 10
        } % 97;
    }
    remainder
}

/// How an account is identified.
///
/// Two forms, because ISO 20022 has two and a bridge that only understood IBANs
/// would refuse every domestic rail that predates them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccountId {
    /// `Id/IBAN`.
    Iban(Iban),
    /// `Id/Othr/Id`: a scheme-specific reference, checked only for length and
    /// for being printable ASCII. There is no checksum to verify — that is the
    /// cost of the form, not an oversight.
    Other(String),
}

impl AccountId {
    /// Parses the proprietary form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for an empty value, one past
    /// 34 characters, or one carrying anything but printable ASCII. Control
    /// characters are refused because this value reaches logs, statements and
    /// operator screens.
    pub fn other(text: &str) -> Result<Self> {
        let invalid = |reason: &str| Error::Invalid {
            field: "Othr/Id",
            reason: format!("{text:?}: {reason}"),
        };
        if text.is_empty() || text.len() > MAX_ACCOUNT_ID_CHARS {
            return Err(invalid("an account identifier is 1 to 34 characters"));
        }
        if !text.bytes().all(|byte| (0x20..0x7f).contains(&byte)) {
            return Err(invalid("an account identifier is printable ASCII"));
        }
        Ok(Self::Other(text.to_owned()))
    }

    /// The identifier as it is written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Iban(iban) => iban.as_str(),
            Self::Other(other) => other,
        }
    }
}

/// A debtor or creditor, as much of one as this bridge acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Party {
    /// `Nm`, if the message carried one.
    pub name: Option<String>,
    /// The account.
    pub account: AccountId,
    /// The party's agent — its bank — if the message named one.
    pub agent: Option<Bic>,
}

impl Party {
    /// A party with an account and nothing else.
    #[must_use]
    pub fn new(account: AccountId) -> Self {
        Self {
            name: None,
            account,
            agent: None,
        }
    }

    /// Checks and attaches a name.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] for a name past [`MAX_NAME_CHARS`].
    pub fn with_name(mut self, name: &str) -> Result<Self> {
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(Error::Invalid {
                field: "Nm",
                reason: format!(
                    "{} characters, limit {MAX_NAME_CHARS}",
                    name.chars().count()
                ),
            });
        }
        self.name = Some(name.to_owned());
        Ok(self)
    }

    /// Attaches an agent.
    #[must_use]
    pub fn with_agent(mut self, agent: Bic) -> Self {
        self.agent = Some(agent);
        self
    }
}

/// A three-letter ISO 4217 currency code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Currency([u8; 3]);

impl Currency {
    /// Parses a currency code.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] unless the value is three uppercase ASCII
    /// letters. Membership of ISO 4217 is not checked: the register changes,
    /// and a bridge refusing a real currency because its table is a year old is
    /// worse than one accepting a code no country uses — which buys nothing,
    /// because the amount scale is fixed either way. See [`crate::amount`].
    pub fn parse(text: &str) -> Result<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 3 || !bytes.iter().all(u8::is_ascii_uppercase) {
            return Err(Error::Invalid {
                field: "Ccy",
                reason: format!("{text:?}: a currency code is three uppercase letters"),
            });
        }
        Ok(Self([bytes[0], bytes[1], bytes[2]]))
    }

    /// The code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Every byte was checked to be an ASCII uppercase letter on the way in.
        std::str::from_utf8(&self.0).unwrap_or("???")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    // Published specimen IBANs from the ISO 13616 registry: real check digits,
    // no real account behind them.
    const VALID: &[&str] = &[
        "GB82WEST12345698765432",
        "DE89370400440532013000",
        "FR1420041010050500013M02606",
        "NO9386011117947",
        "MT84MALT011000012345MTLCAST001S",
    ];

    #[test]
    fn published_specimen_ibans_pass_the_checksum() {
        for text in VALID {
            Iban::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"));
        }
    }

    #[test]
    fn a_transposed_pair_of_digits_fails_the_checksum() {
        // The failure mod-97 exists to catch, and the reason this crate does
        // the arithmetic rather than checking the shape and moving on.
        assert!(Iban::parse("GB82WEST12345698765423").is_err());
        assert!(Iban::parse("DE89370400440532013lie".to_uppercase().as_str()).is_err());
    }

    #[test]
    fn a_changed_check_digit_fails() {
        assert!(Iban::parse("GB83WEST12345698765432").is_err());
    }

    #[test]
    fn an_iban_of_the_wrong_shape_is_refused_before_the_checksum() {
        for text in [
            "",
            "GB82",
            "gb82west12345698765432",
            "GB82 WEST 1234 5698 7654 32",
            "GB82WEST12345698765432GB82WEST12345698765432",
            "1282WEST12345698765432",
        ] {
            assert!(Iban::parse(text).is_err(), "{text} parsed");
        }
    }

    #[test]
    fn a_bic_is_eight_or_eleven_uppercase_characters() {
        Bic::parse("DEUTDEFF").expect("8");
        Bic::parse("DEUTDEFF500").expect("11");
        for text in ["DEUTDEF", "DEUTDEFF5", "deutdeff", "DEUT-DEFF", ""] {
            assert!(Bic::parse(text).is_err(), "{text} parsed");
        }
    }

    #[test]
    fn a_proprietary_account_id_refuses_control_characters() {
        AccountId::other("ACCT-000123").expect("printable");
        for text in ["", "acct\u{0}id", "acct\nid", &"9".repeat(35)] {
            assert!(AccountId::other(text).is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn a_currency_is_three_uppercase_letters() {
        assert_eq!(Currency::parse("EUR").expect("EUR").as_str(), "EUR");
        for text in ["", "EU", "EURO", "eur", "E1R"] {
            assert!(Currency::parse(text).is_err(), "{text} parsed");
        }
    }

    #[test]
    fn a_name_past_the_schema_limit_is_refused() {
        let party = Party::new(AccountId::other("ACCT-1").expect("id"));
        assert!(party.clone().with_name(&"a".repeat(MAX_NAME_CHARS)).is_ok());
        assert!(party.with_name(&"a".repeat(MAX_NAME_CHARS + 1)).is_err());
    }
}
