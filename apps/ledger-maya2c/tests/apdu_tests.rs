//! The APDU protocol layer, on the host.
//!
//! No device, no emulator. Everything here is the chunk-assembly state machine
//! and path validation — which is where the bugs are, and which needs neither.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use app_maya2c::apdu::{
    ApduError, Assembler, CLA, Chunk, Command, HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LEN,
    Instruction, MAX_APDU_PAYLOAD, MAX_TX_BYTES, Pages, parse,
};
use app_maya2c::derive::{COIN_TYPE, DerivationPath, HARDENED, PATH_LEN};

/// Builds a frame with a correct `Lc`.
fn frame(ins: u8, p1: u8, p2: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![CLA, ins, p1, p2, payload.len() as u8];
    out.extend_from_slice(payload);
    out
}

fn command<'a>(chunk: Chunk, p2: u8, payload: &'a [u8]) -> Command<'a> {
    Command {
        instruction: Instruction::SignTransaction,
        chunk,
        p2,
        payload,
    }
}

// ---------------------------------------------------------------------------
// framing
// ---------------------------------------------------------------------------

#[test]
fn a_well_formed_frame_parses() {
    let f = frame(0x04, 0x00, 0x00, b"payload");
    let parsed = parse(&f).expect("valid frame");

    assert_eq!(parsed.instruction, Instruction::SignTransaction);
    assert_eq!(parsed.chunk, Chunk::First);
    assert_eq!(parsed.payload, b"payload");
}

#[test]
fn a_declared_length_that_disagrees_is_refused() {
    // The check that matters most in this file. A host that under-declares `Lc`
    // and appends extra bytes would otherwise have the device sign over content
    // it never counted -- and the approval screen would describe something
    // else.
    let mut f = frame(0x04, 0x00, 0x00, b"four");
    f.extend_from_slice(b"extra");

    assert_eq!(
        parse(&f),
        Err(ApduError::LengthMismatch {
            declared: 4,
            actual: 9
        })
    );
}

#[test]
fn a_foreign_class_byte_is_refused() {
    let mut f = frame(0x04, 0x00, 0x00, b"x");
    f[0] = 0xB0;
    assert_eq!(parse(&f), Err(ApduError::BadClass(0xB0)));
}

#[test]
fn a_short_header_is_refused_rather_than_indexed() {
    for len in 0..5 {
        assert_eq!(parse(&vec![CLA; len]), Err(ApduError::Truncated), "{len}");
    }
}

#[test]
fn an_unknown_instruction_is_refused() {
    let f = frame(0x99, 0x00, 0x00, b"");
    assert_eq!(parse(&f), Err(ApduError::UnknownInstruction(0x99)));
}

#[test]
fn every_error_carries_a_distinct_enough_status_word() {
    // A host has to tell "resend this frame" from "restart the sequence" from
    // "your path is wrong" without parsing prose.
    assert_eq!(ApduError::BadClass(0).status_word(), 0x6E00);
    assert_eq!(ApduError::UnknownInstruction(0).status_word(), 0x6D00);
    assert_eq!(ApduError::BadDerivationPath.status_word(), 0x6A80);
    assert_ne!(
        ApduError::Truncated.status_word(),
        ApduError::NoSequenceInProgress.status_word(),
    );
}

// ---------------------------------------------------------------------------
// assembly
// ---------------------------------------------------------------------------

#[test]
fn a_sequence_reassembles_in_order() {
    let mut asm = Assembler::new();

    assert_eq!(asm.push(&command(Chunk::First, 0, b"abc")), Ok(None));
    assert_eq!(asm.push(&command(Chunk::More, 1, b"def")), Ok(None));

    let done = asm
        .push(&command(Chunk::Last, 2, b"ghi"))
        .expect("accepted")
        .expect("complete");
    assert_eq!(done, b"abcdefghi");
}

#[test]
fn a_continuation_with_nothing_started_is_refused() {
    let mut asm = Assembler::new();
    assert_eq!(
        asm.push(&command(Chunk::More, 0, b"x")),
        Err(ApduError::NoSequenceInProgress)
    );
}

#[test]
fn a_gap_in_the_sequence_is_refused_and_discards_everything() {
    // The property worth having. A dropped chunk that was silently appended
    // would produce a signature over a payload the host never sent, and the
    // user would have approved a screen describing the payload they meant.
    let mut asm = Assembler::new();
    asm.push(&command(Chunk::First, 0, b"abc")).expect("first");

    assert_eq!(
        asm.push(&command(Chunk::More, 2, b"ghi")),
        Err(ApduError::OutOfOrder {
            expected: 1,
            received: 2
        })
    );

    // Not resumable. A rejected sequence that could be continued is a sequence
    // a host can splice two payloads into.
    assert!(!asm.in_progress());
    assert!(asm.is_empty());
    assert_eq!(
        asm.push(&command(Chunk::More, 1, b"def")),
        Err(ApduError::NoSequenceInProgress)
    );
}

#[test]
fn a_repeated_chunk_index_is_refused() {
    let mut asm = Assembler::new();
    asm.push(&command(Chunk::First, 0, b"abc")).expect("first");
    assert!(matches!(
        asm.push(&command(Chunk::More, 0, b"abc")),
        Err(ApduError::OutOfOrder { .. })
    ));
}

#[test]
fn a_new_first_chunk_discards_a_partial_sequence() {
    // Restarting is the documented way out of a failed transfer, so it has to
    // leave nothing of the old payload behind.
    let mut asm = Assembler::new();
    asm.push(&command(Chunk::First, 0, b"stale"))
        .expect("first");

    asm.push(&command(Chunk::First, 0, b"fresh"))
        .expect("restart");
    let done = asm
        .push(&command(Chunk::Last, 1, b""))
        .expect("finish")
        .expect("complete");
    assert_eq!(done, b"fresh");
}

#[test]
fn a_payload_past_the_ceiling_is_refused() {
    let mut asm = Assembler::new();
    let page = [0u8; MAX_APDU_PAYLOAD];

    let mut index = 0u16;
    let mut chunk = Chunk::First;
    loop {
        match asm.push(&command(chunk, index as u8, &page)) {
            Ok(_) => {}
            Err(ApduError::TooLarge { limit, .. }) => {
                assert_eq!(limit, MAX_TX_BYTES);
                assert!(asm.is_empty(), "a refused sequence must hold nothing");
                return;
            }
            Err(other) => panic!("unexpected error before the ceiling: {other:?}"),
        }
        chunk = Chunk::More;
        index = index.wrapping_add(1);
        assert!(asm.len() <= MAX_TX_BYTES);
    }
}

// ---------------------------------------------------------------------------
// paging
// ---------------------------------------------------------------------------

#[test]
fn a_signature_pages_into_the_expected_number_of_responses() {
    // The number that makes this protocol awkward, asserted rather than
    // assumed: 11,165 bytes over a 255-byte pipe is 44 responses.
    assert_eq!(Pages::count_for(HYBRID_SIGNATURE_LEN), 44);
    assert_eq!(Pages::count_for(HYBRID_PUBLIC_KEY_LEN), 8);

    let signature = vec![0xABu8; HYBRID_SIGNATURE_LEN];
    let pages: Vec<&[u8]> = Pages::new(&signature).collect();

    assert_eq!(pages.len(), 44);
    assert!(pages.iter().all(|p| p.len() <= MAX_APDU_PAYLOAD));
    assert_eq!(
        pages.iter().map(|p| p.len()).sum::<usize>(),
        HYBRID_SIGNATURE_LEN
    );
}

#[test]
fn paging_nothing_yields_nothing() {
    assert_eq!(Pages::count_for(0), 0);
    assert_eq!(Pages::new(&[]).count(), 0);
}

#[test]
fn an_exact_multiple_does_not_produce_a_trailing_empty_page() {
    let data = vec![0u8; MAX_APDU_PAYLOAD * 3];
    assert_eq!(Pages::new(&data).count(), 3);
}

// ---------------------------------------------------------------------------
// derivation paths
// ---------------------------------------------------------------------------

fn path_bytes(components: &[u32]) -> Vec<u8> {
    let mut out = vec![components.len() as u8];
    for c in components {
        out.extend_from_slice(&c.to_be_bytes());
    }
    out
}

fn valid_path(account: u32, index: u32) -> Vec<u8> {
    path_bytes(&[
        44 | HARDENED,
        COIN_TYPE | HARDENED,
        account | HARDENED,
        HARDENED,
        index | HARDENED,
    ])
}

#[test]
fn the_standard_path_parses() {
    let parsed = DerivationPath::parse(&valid_path(0, 3)).expect("valid");
    assert_eq!(parsed.account, 0);
    assert_eq!(parsed.index, 3);
    assert_eq!(parsed.components()[1], COIN_TYPE | HARDENED);
}

#[test]
fn another_chains_coin_type_is_refused() {
    // Bitcoin's path. Accepting it would hand back a key whose address this
    // chain has never seen, with nothing reporting a problem.
    let bitcoin = path_bytes(&[
        44 | HARDENED,
        HARDENED, // coin type 0'
        HARDENED,
        HARDENED,
        HARDENED,
    ]);
    assert_eq!(
        DerivationPath::parse(&bitcoin),
        Err(ApduError::BadDerivationPath)
    );
}

#[test]
fn an_unhardened_component_is_refused() {
    // A leaked unhardened child key can be walked back to its siblings.
    let soft = path_bytes(&[44 | HARDENED, COIN_TYPE | HARDENED, 0, HARDENED, HARDENED]);
    assert_eq!(
        DerivationPath::parse(&soft),
        Err(ApduError::BadDerivationPath)
    );
}

#[test]
fn a_path_of_the_wrong_length_is_refused() {
    for len in [0usize, 1, 4, 6] {
        let components: Vec<u32> = (0..len).map(|_| HARDENED).collect();
        assert_eq!(
            DerivationPath::parse(&path_bytes(&components)),
            Err(ApduError::BadDerivationPath),
            "length {len}"
        );
    }
    assert_eq!(PATH_LEN, 5);
}

#[test]
fn a_count_byte_that_lies_about_the_body_is_refused() {
    let mut bytes = valid_path(0, 0);
    bytes[0] = 4;
    assert_eq!(
        DerivationPath::parse(&bytes),
        Err(ApduError::BadDerivationPath)
    );
}
