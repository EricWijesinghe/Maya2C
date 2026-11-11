//! Payee check: who a transfer goes to, said in words before the signature.
//!
//! The confirm screen already shows the grouped address and the review
//! warnings. This adds what a person needs to recognise the payee: the saved
//! name, whether the payee is a first-time one, and whether the address
//! imitates a saved contact. The native approval dialog carries these lines,
//! and the webview never supplies them.

use std::collections::BTreeSet;

use maya_clear_sign::Network;

use crate::address_book::AddressBook;

/// What the book and this session's payments say about a recipient.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayeeCheck {
    /// The recipient in full grouped Bech32m, the form the signer compares.
    pub grouped: String,
    /// The saved name, when the address is in the address book.
    pub contact: Option<String>,
    /// A saved contact this address imitates without being it.
    pub look_alike_of: Option<String>,
    /// Never saved and never paid from this wallet.
    pub first_time: bool,
}

/// Checks `address` against the book and the addresses already paid (`paid`).
#[must_use]
pub fn payee_check(
    book: &AddressBook,
    address: &[u8; 32],
    network: Network,
    paid: &BTreeSet<[u8; 32]>,
) -> PayeeCheck {
    let contact = book.find(address).map(|c| c.name.clone());
    PayeeCheck {
        grouped: maya_clear_sign::grouped(network, address),
        look_alike_of: book.look_alike_of(address).map(|c| c.name.clone()),
        first_time: contact.is_none() && !paid.contains(address),
        contact,
    }
}

/// The payee lines for the approval dialog, one per fact, in reading order.
#[must_use]
pub fn payee_lines(check: &PayeeCheck) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(match &check.contact {
        Some(name) => format!("Payee: {name} (saved contact)"),
        None => "Payee: not in your contacts".to_string(),
    });
    if check.first_time {
        lines.push("First payment to this address. Check every group of it.".to_string());
    }
    if let Some(name) = &check.look_alike_of {
        lines.push(format!("Looks like {name} in your contacts but is a different address. Do not send to it unless you chose it."));
    }
    lines
}

/// `message` with the payee lines placed above it.
#[must_use]
pub fn with_payee(check: &PayeeCheck, message: &str) -> String {
    let mut text = payee_lines(check).join("\n");
    text.push('\n');
    text.push_str(message);
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use primitive_types::U256;

    use super::*;
    use crate::address_book::Source;
    use crate::amount::format_display;

    fn book_with(address: [u8; 32], name: &str) -> AddressBook {
        let mut book = AddressBook::default();
        book.add(address, Network::Testnet, name, "", Source::Manual, 1)
            .unwrap();
        book
    }

    /// Same first two and last two bytes, different address: a look-alike.
    fn imitation(address: [u8; 32]) -> [u8; 32] {
        let mut other = address;
        other[15] ^= 0xff;
        other
    }

    #[test]
    fn known_payee_shows_the_saved_name() {
        // Arrange
        let alice = [0x11; 32];
        let book = book_with(alice, "Alice");
        // Act
        let check = payee_check(&book, &alice, Network::Testnet, &BTreeSet::new());
        // Assert
        assert_eq!(check.contact.as_deref(), Some("Alice"));
        assert!(!check.first_time);
        assert!(check.look_alike_of.is_none());
        assert!(payee_lines(&check).contains(&"Payee: Alice (saved contact)".to_string()));
    }

    #[test]
    fn first_time_payee_is_flagged() {
        // Arrange
        let stranger = [0x22; 32];
        let book = AddressBook::default();
        // Act
        let check = payee_check(&book, &stranger, Network::Testnet, &BTreeSet::new());
        // Assert
        assert!(check.first_time);
        assert!(check.contact.is_none());
        let lines = payee_lines(&check);
        assert!(lines.contains(&"Payee: not in your contacts".to_string()));
        assert!(lines.iter().any(|l| l.starts_with("First payment to this address")));
    }

    #[test]
    fn paid_before_but_not_saved_is_not_first_time() {
        // Arrange
        let stranger = [0x33; 32];
        let paid = BTreeSet::from([stranger]);
        // Act
        let check = payee_check(&AddressBook::default(), &stranger, Network::Testnet, &paid);
        // Assert
        assert!(!check.first_time);
        assert!(check.contact.is_none());
    }

    #[test]
    fn look_alike_of_a_contact_is_flagged() {
        // Arrange
        let alice = [0x44; 32];
        let fake = imitation(alice);
        let book = book_with(alice, "Alice");
        // Act
        let check = payee_check(&book, &fake, Network::Testnet, &BTreeSet::new());
        // Assert
        assert_eq!(check.look_alike_of.as_deref(), Some("Alice"));
        assert!(check.contact.is_none());
        assert!(payee_lines(&check).iter().any(|l| l.starts_with("Looks like Alice")));
    }

    #[test]
    fn with_payee_puts_the_payee_above_the_message() {
        // Arrange
        let check = payee_check(&AddressBook::default(), &[0x55; 32], Network::Testnet, &BTreeSet::new());
        // Act
        let text = with_payee(&check, "Amount: 1 MAYA");
        // Assert
        assert!(text.ends_with("\nAmount: 1 MAYA"));
        assert!(text.starts_with("Payee: not in your contacts"));
    }

    #[test]
    fn amount_line_keeps_all_24_places_grouped() {
        // Arrange: 1,234.5 coins in base units.
        let coins = U256::from(12_345_u64) * U256::exp10(23);
        // Act
        let shown = format_display(coins);
        // Assert
        assert_eq!(shown, "1,234.500000000000000000000000");
        assert_eq!(shown.split('.').nth(1).unwrap().len(), 24);
    }
}
