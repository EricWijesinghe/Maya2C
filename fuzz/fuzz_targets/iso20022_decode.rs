//! Fuzzes the ISO 20022 readers: XML framing, the three message types, and the
//! amount parser.
//!
//! These bytes arrive on a bank rail from whatever dialled it, so the property
//! is total: any input is either a message this crate can state in full, or a
//! refusal. It never panics and never allocates past the bounds in
//! `maya_iso20022::xml`.
//!
//! Two properties beyond "does not crash", because a decoder that merely
//! survives is not the bar:
//!
//! - **A parsed message re-renders and reparses to itself.** If a document
//!   decodes, the value it decoded to has to survive a round trip. A field the
//!   writer drops is a payment instruction that changes when it is forwarded.
//! - **A camt.053 that parses has balances its own entries produce.** The
//!   reader checks it on the way in, so a document that got past the reader and
//!   then fails the same arithmetic here would mean the two disagree.

#![no_main]

#![allow(clippy::unwrap_used, clippy::expect_used)]

use libfuzzer_sys::fuzz_target;
use maya_iso20022::{amount::Amount, camt053, pacs008, pacs009, xml};

fuzz_target!(|data: &[u8]| {
    // The generic reader, which every message type goes through.
    if let Ok(root) = xml::parse(data) {
        // Rendering what parsing produced must itself parse. The namespace is
        // irrelevant to the reader — it matches on local names — so any value
        // will do here.
        if let Ok(rendered) = xml::render(&root, "urn:fuzz") {
            let reparsed = xml::parse(rendered.as_bytes()).expect("rendered output must reparse");
            assert_eq!(
                reparsed.name, root.name,
                "the root element changed across a render"
            );
        }
    }

    if let Ok(message) = pacs008::parse(data) {
        let rendered = message.to_xml().expect("a parsed message must render");
        let reparsed = pacs008::parse(rendered.as_bytes()).expect("and reparse");
        assert_eq!(reparsed, message, "pacs.008 did not survive a round trip");
    }

    if let Ok(message) = pacs009::parse(data) {
        let rendered = message.to_xml().expect("a parsed message must render");
        let reparsed = pacs009::parse(rendered.as_bytes()).expect("and reparse");
        assert_eq!(reparsed, message, "pacs.009 did not survive a round trip");
    }

    if let Ok(statement) = camt053::parse(data) {
        let rendered = statement.to_xml().expect("a parsed statement must render");
        let reparsed = camt053::parse(rendered.as_bytes()).expect("and reparse");
        assert_eq!(reparsed, statement, "camt.053 did not survive a round trip");

        // The reader re-derives the closing balance and refuses a mismatch, so
        // anything that got this far has to satisfy it again.
        let mut running = i128::from(statement.opening.base_units());
        for entry in &statement.entries {
            let amount = i128::from(entry.amount.base_units());
            running += match entry.direction {
                camt053::Direction::Credit => amount,
                camt053::Direction::Debit => -amount,
            };
        }
        assert_eq!(
            running,
            i128::from(statement.closing.base_units()),
            "a statement passed the reader with balances its entries do not produce"
        );
    }

    // The amount parser on its own, over whatever the input spells. It decides
    // how much money moves, so it gets its own pass rather than only the one it
    // receives inside a well-formed document.
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(amount) = Amount::parse(text) {
            let rendered = amount.to_iso_decimal();
            let reparsed = Amount::parse(&rendered).expect("a rendered amount must reparse");
            assert_eq!(
                reparsed, amount,
                "{text:?} rendered as {rendered:?} and read back as a different amount"
            );
        }
    }
});
