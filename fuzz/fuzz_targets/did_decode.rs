//! Fuzzes the identity decoders: DID strings, documents, attestations and
//! revocation pages.
//!
//! A DID Document arrives from a stranger — resolved from another node, handed
//! over by a wallet, read out of a QR code — so the property is total: any
//! input is refused or understood, never a panic and never an unbounded
//! allocation.
//!
//! Two properties beyond survival:
//!
//! - **A decoded record re-encodes to itself.** A record that changed on the way
//!   through would be a resolver that altered what it served, and every
//!   consumer downstream would be reading a different document from the one the
//!   chain committed.
//! - **A parsed DID renders back to the string it came from.** Two spellings of
//!   one identifier is a subject an attacker can impersonate by finding the
//!   second.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_identity::attestation::{CryptographicAttestation, RevocationPage};
use maya_identity::did::Did;
use maya_identity::document::DidDocument;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data)
        && let Ok(did) = Did::parse(text)
    {
        // Canonical: the identifier this parsed to renders to exactly the text
        // that produced it. If it did not, two strings would name one subject.
        assert_eq!(did.to_string(), text, "a DID has two spellings");
        assert_eq!(Did::parse(&did.to_string()).expect("re-parse"), did);
    }

    if let Ok(document) = DidDocument::decode(data) {
        let again = DidDocument::decode(&document.encode()).expect("a decoded document re-decodes");
        assert_eq!(again, document, "a document changed across a round trip");
    }

    if let Ok(attestation) = CryptographicAttestation::decode(data) {
        let again = CryptographicAttestation::decode(&attestation.encode())
            .expect("a decoded attestation re-decodes");
        assert_eq!(again, attestation, "an attestation changed across a round trip");
        // An attestation that expires before it was anchored should never have
        // decoded; the constructor is the only way to build one.
        if let Some(expiry) = attestation.expires_at {
            assert!(expiry > attestation.anchored_at);
        }
    }

    if let Ok(page) = RevocationPage::decode(data) {
        let again = RevocationPage::decode(&page.encode()).expect("a decoded page re-decodes");
        assert_eq!(again, page, "a page changed across a round trip");
        // Fail closed: an index past the page reads as revoked, never as live.
        assert!(page.is_revoked(maya_identity::attestation::BITS_PER_PAGE));
    }
});
