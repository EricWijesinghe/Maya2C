//! Turning an account identifier into the 32 bytes a sanctions list holds.
//!
//! ## Why this is here and the proof is not
//!
//! The proof lives in `maya_zk_privacy::sanctions`, which owns the field, the
//! Poseidon hash and the tree. This module owns the one thing that crate must
//! never need to know: what an IBAN is.
//!
//! The split is not tidiness. `zk-privacy` pulls in the whole arkworks stack,
//! and this crate's whole point is to be light enough to fuzz an XML decoder in
//! — so the seam between them is 32 opaque bytes, produced here and consumed
//! there.
//!
//! ## Canonicalisation is the security property
//!
//! A digest is only a useful list key if one account has exactly one digest.
//! Two spellings of the same IBAN that hash differently is a sanctioned party
//! with a second identity, and a list that cannot catch them.
//!
//! So the input is canonicalised before hashing, and the canonicalisation is
//! **rejection, not repair**:
//!
//! - The [`crate::party`] types already refuse lowercase, embedded spaces and
//!   malformed check digits, so a value that reaches here has one spelling.
//! - The scheme is folded into the digest as a domain, so an IBAN and a
//!   proprietary identifier with the same characters are different entries.
//!
//! Repairing instead — upcasing, stripping spaces — would mean the list keyed
//! on a value nobody sent, and two parsers disagreeing about which repairs to
//! apply would key on two different values.
//!
//! ## What a digest is not
//!
//! Not a secret. BLAKE3 of a structured 34-character string is guessable by
//! anyone willing to enumerate, and a published list is published. Privacy in
//! this design comes from the *proof* — the verifier never sees the digest at
//! all — not from the digest being hard to invert. A design that leaked the
//! digest and called it anonymised would be wrong; see
//! `maya_zk_privacy::sanctions`.

use crate::party::{AccountId, Bic};

/// Bytes in a list identifier, matching `maya_zk_privacy::sanctions::Identifier`.
pub const IDENTIFIER_BYTES: usize = 32;

/// The 32 bytes a sanctions list holds for one party.
pub type Identifier = [u8; IDENTIFIER_BYTES];

/// Domain for an IBAN-keyed entry.
const DOMAIN_IBAN: &[u8] = b"maya-iso20022-sanctions-v1:iban:";

/// Domain for a proprietary account identifier.
const DOMAIN_OTHER: &[u8] = b"maya-iso20022-sanctions-v1:othr:";

/// Domain for an institution keyed by BIC.
const DOMAIN_BIC: &[u8] = b"maya-iso20022-sanctions-v1:bic:";

/// The list identifier for an account.
///
/// The two forms are separately domained: an IBAN and a proprietary reference
/// that happen to spell the same characters are different accounts, and one
/// digest for both would let a listing of one silently cover the other.
#[must_use]
pub fn identifier_for_account(account: &AccountId) -> Identifier {
    let (domain, value) = match account {
        AccountId::Iban(iban) => (DOMAIN_IBAN, iban.as_str()),
        AccountId::Other(other) => (DOMAIN_OTHER, other.as_str()),
    };
    digest(domain, value.as_bytes())
}

/// The list identifier for an institution.
#[must_use]
pub fn identifier_for_institution(bic: &Bic) -> Identifier {
    digest(DOMAIN_BIC, bic.as_str().as_bytes())
}

/// Domain-separated BLAKE3.
///
/// The domain is hashed as a prefix rather than concatenated with a separator
/// character, because every domain here ends in `:` and no identifier may
/// contain one — the [`crate::party`] charsets are letters and digits, so the
/// boundary is unambiguous without a length prefix.
fn digest(domain: &[u8], value: &[u8]) -> Identifier {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(value);
    *hasher.finalize().as_bytes()
}

/// Every party a payment must clear before it settles.
///
/// Both ends, and both agents when the message named them. A bridge that
/// checked only the creditor would let a sanctioned debtor pay anyone, which is
/// the direction sanctions are usually written to stop.
#[must_use]
pub fn identifiers_for_payment(intent: &crate::bridge::PaymentIntent) -> Vec<Identifier> {
    let mut identifiers = vec![
        identifier_for_account(&intent.debtor.account),
        identifier_for_account(&intent.creditor.account),
    ];
    for agent in [intent.debtor.agent.as_ref(), intent.creditor.agent.as_ref()]
        .into_iter()
        .flatten()
    {
        identifiers.push(identifier_for_institution(agent));
    }
    identifiers
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::party::Iban;

    #[test]
    fn one_account_has_one_identifier() {
        let account = AccountId::Iban(Iban::parse("GB82WEST12345698765432").expect("iban"));
        assert_eq!(
            identifier_for_account(&account),
            identifier_for_account(&account)
        );
    }

    #[test]
    fn two_accounts_have_different_identifiers() {
        let first = AccountId::Iban(Iban::parse("GB82WEST12345698765432").expect("iban"));
        let second = AccountId::Iban(Iban::parse("DE89370400440532013000").expect("iban"));
        assert_ne!(
            identifier_for_account(&first),
            identifier_for_account(&second)
        );
    }

    #[test]
    fn an_iban_and_a_proprietary_reference_spelling_the_same_thing_differ() {
        // The reason the two forms are separately domained. Without it, listing
        // an account by one form would silently cover the other — or worse,
        // fail to, depending on which form the sender used.
        let iban = AccountId::Iban(Iban::parse("GB82WEST12345698765432").expect("iban"));
        let other = AccountId::other("GB82WEST12345698765432").expect("other");
        assert_ne!(
            identifier_for_account(&iban),
            identifier_for_account(&other)
        );
    }

    #[test]
    fn an_account_and_an_institution_spelling_the_same_thing_differ() {
        let account = AccountId::other("DEUTDEFF").expect("other");
        let bic = Bic::parse("DEUTDEFF").expect("bic");
        assert_ne!(
            identifier_for_account(&account),
            identifier_for_institution(&bic)
        );
    }

    #[test]
    fn a_payment_yields_both_ends_and_both_agents() {
        use crate::amount::Amount;
        use crate::bridge::{Origin, PaymentIntent};
        use crate::party::{Currency, Party};

        let debtor = Party::new(AccountId::other("ACCT-1").expect("id"))
            .with_agent(Bic::parse("DEUTDEFF").expect("bic"));
        let creditor = Party::new(AccountId::other("ACCT-2").expect("id"))
            .with_agent(Bic::parse("BNPAFRPP").expect("bic"));
        let intent = PaymentIntent {
            origin: Origin::CustomerCreditTransfer,
            message_id: "MSG-1".into(),
            end_to_end_id: "E2E-1".into(),
            debtor,
            creditor,
            amount: Amount::from_base_units(100),
            currency: Currency::parse("EUR").expect("ccy"),
            remittance: None,
        };

        let identifiers = identifiers_for_payment(&intent);
        assert_eq!(identifiers.len(), 4);
        // All four distinct: an identifier appearing twice would mean one
        // listing covering a party it was never meant to.
        let mut sorted = identifiers.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 4);
    }

    #[test]
    fn a_payment_without_agents_yields_only_the_two_accounts() {
        use crate::amount::Amount;
        use crate::bridge::{Origin, PaymentIntent};
        use crate::party::{Currency, Party};

        let intent = PaymentIntent {
            origin: Origin::InstitutionTransfer,
            message_id: "MSG-1".into(),
            end_to_end_id: "E2E-1".into(),
            debtor: Party::new(AccountId::other("ACCT-1").expect("id")),
            creditor: Party::new(AccountId::other("ACCT-2").expect("id")),
            amount: Amount::from_base_units(100),
            currency: Currency::parse("EUR").expect("ccy"),
            remittance: None,
        };
        assert_eq!(identifiers_for_payment(&intent).len(), 2);
    }
}
