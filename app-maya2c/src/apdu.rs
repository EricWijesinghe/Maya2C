//! APDU framing for the Maya2C Ledger app.
//!
//! # The transport is the hard part, not the crypto
//!
//! An APDU carries at most 255 bytes of payload. Maya2C's objects do not fit:
//!
//! | Object | Bytes | APDUs |
//! |---|---|---|
//! | hybrid public key | 1,984 | 8 out |
//! | hybrid signature | 11,165 | 44 out |
//! | a signed transfer | ~13,000 | 52 in |
//!
//! So every command is a sequence, and the sequence is where the bugs are: a
//! chunk accepted out of order, a length that disagrees with what arrived, a
//! continuation that silently starts a new transaction. None of that needs a
//! device to get wrong, and none of it needs a device to test — which is why
//! this module is `no_std` but host-testable, and why it holds no key material.
//!
//! # Chunks are counted, not trusted
//!
//! Each chunk carries its own index. The assembler rejects a chunk that is not
//! the one it expects rather than appending it, because a host that drops a
//! chunk and continues would otherwise get a signature over a payload it never
//! sent — and the user would have approved a screen describing something else.

#![allow(clippy::module_name_repetitions)]

/// Instruction class for this application.
///
/// `0xE0` is the conventional CLA for a Ledger app; the device dispatches on it
/// before this code sees the frame.
pub const CLA: u8 = 0xE0;

/// Largest payload one APDU can carry.
pub const MAX_APDU_PAYLOAD: usize = 255;

/// Hybrid public key length, from `src/crypto/hybrid.rs`.
///
/// Duplicated rather than imported: importing would mean depending on
/// `custom-l1-node`, which pulls RocksDB's C++ into a Cortex-M binary. The
/// duplication is pinned by a parity test instead.
pub const HYBRID_PUBLIC_KEY_LEN: usize = 1984;

/// Hybrid signature length: ML-DSA-65 (3,309) + SLH-DSA-SHA2-128s (7,856).
pub const HYBRID_SIGNATURE_LEN: usize = 11165;

/// ML-DSA-65 signature length on its own.
///
/// The device can produce this half. It is **not a valid Maya2C signature** —
/// `HybridVerifyingKey::verify` checks both halves — and the constant exists so
/// the measurement has something to name, not because a transaction can be
/// built from it.
pub const ML_DSA_SIGNATURE_LEN: usize = 3309;

/// Largest transaction the device will accept for signing.
///
/// A ceiling on RAM the host can make the device commit. 16 KiB is comfortably
/// past a single transfer (~13 KB, dominated by the 11,165-byte signature the
/// *previous* signer attached) while still being a bound rather than a wish.
pub const MAX_TX_BYTES: usize = 16 * 1024;

/// Commands this application answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Instruction {
    /// Return the hybrid public key for a derivation path.
    GetPublicKey = 0x02,
    /// Sign a transaction, streamed in over several APDUs.
    SignTransaction = 0x04,
    /// Show the address on screen for the user to compare.
    DisplayAddress = 0x06,
}

impl Instruction {
    /// Parses an instruction byte.
    ///
    /// # Errors
    ///
    /// [`ApduError::UnknownInstruction`] for anything not listed above. An
    /// unrecognised instruction is refused rather than ignored: a host talking
    /// to the wrong app should learn that immediately.
    pub const fn from_byte(byte: u8) -> Result<Self, ApduError> {
        match byte {
            0x02 => Ok(Self::GetPublicKey),
            0x04 => Ok(Self::SignTransaction),
            0x06 => Ok(Self::DisplayAddress),
            other => Err(ApduError::UnknownInstruction(other)),
        }
    }
}

/// Where a chunk sits in a sequence.
///
/// Carried in P1. `First` resets the assembler; `More` and `Last` require one
/// already in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chunk {
    /// Begins a new payload, discarding any partial one.
    First,
    /// Continues the payload in progress.
    More,
    /// Completes the payload in progress.
    Last,
}

impl Chunk {
    /// Parses the P1 byte.
    ///
    /// # Errors
    ///
    /// [`ApduError::BadParameter`] for any other value.
    pub const fn from_p1(p1: u8) -> Result<Self, ApduError> {
        match p1 {
            0x00 => Ok(Self::First),
            0x01 => Ok(Self::More),
            0x02 => Ok(Self::Last),
            other => Err(ApduError::BadParameter(other)),
        }
    }
}

/// Anything wrong with a frame or a sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApduError {
    /// Not this application's class byte.
    BadClass(u8),
    /// Instruction byte names no command.
    UnknownInstruction(u8),
    /// P1 or P2 outside its permitted set.
    BadParameter(u8),
    /// Header shorter than the five bytes every APDU has.
    Truncated,
    /// Declared length disagrees with the bytes that followed.
    LengthMismatch {
        /// What the header declared.
        declared: usize,
        /// What actually arrived.
        actual: usize,
    },
    /// A continuation arrived with no sequence in progress.
    NoSequenceInProgress,
    /// A chunk arrived out of order.
    ///
    /// Kept distinct from a length problem because the remedy differs: the host
    /// must restart the sequence, not resend one frame.
    OutOfOrder {
        /// The index the device expected next.
        expected: u16,
        /// The index that arrived.
        received: u16,
    },
    /// The accumulated payload would exceed [`MAX_TX_BYTES`].
    TooLarge {
        /// Bytes that would have been held.
        needed: usize,
        /// The ceiling.
        limit: usize,
    },
    /// A derivation path that is not `m/44'/7331'/a'/0'/i'`.
    BadDerivationPath,
}

impl ApduError {
    /// The ISO 7816 status word a host sees.
    ///
    /// Distinct words per class, so a host can tell "resend" from "restart the
    /// sequence" from "your path is wrong" without parsing a message.
    #[must_use]
    pub const fn status_word(self) -> u16 {
        match self {
            Self::BadClass(_) => 0x6E00,
            Self::UnknownInstruction(_) => 0x6D00,
            Self::BadParameter(_) => 0x6B00,
            Self::Truncated | Self::LengthMismatch { .. } => 0x6700,
            Self::NoSequenceInProgress | Self::OutOfOrder { .. } => 0x6985,
            Self::TooLarge { .. } => 0x6A84,
            Self::BadDerivationPath => 0x6A80,
        }
    }
}

/// A parsed APDU header and its payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command<'a> {
    /// Which command.
    pub instruction: Instruction,
    /// Position in the sequence.
    pub chunk: Chunk,
    /// P2, used as the low byte of the chunk index.
    pub p2: u8,
    /// This frame's payload.
    pub payload: &'a [u8],
}

/// Parses one APDU frame.
///
/// Layout is the standard `CLA INS P1 P2 Lc <payload>`.
///
/// # Errors
///
/// [`ApduError::Truncated`] for a short header, [`ApduError::BadClass`] for a
/// foreign CLA, and [`ApduError::LengthMismatch`] when `Lc` disagrees with what
/// followed — checked rather than trusted, because a host that under-declares
/// would otherwise have the device sign over bytes it did not count.
pub fn parse(frame: &[u8]) -> Result<Command<'_>, ApduError> {
    if frame.len() < 5 {
        return Err(ApduError::Truncated);
    }
    if frame[0] != CLA {
        return Err(ApduError::BadClass(frame[0]));
    }

    let instruction = Instruction::from_byte(frame[1])?;
    let chunk = Chunk::from_p1(frame[2])?;
    let declared = frame[4] as usize;
    let payload = &frame[5..];

    if payload.len() != declared {
        return Err(ApduError::LengthMismatch {
            declared,
            actual: payload.len(),
        });
    }

    Ok(Command {
        instruction,
        chunk,
        p2: frame[3],
        payload,
    })
}

/// Reassembles a payload streamed over several APDUs.
///
/// Fixed capacity, no allocation: this runs on a device with no heap, and a
/// growable buffer would be a way for a host to exhaust it.
pub struct Assembler {
    buffer: [u8; MAX_TX_BYTES],
    len: usize,
    next_index: u16,
    in_progress: bool,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Assembler {
    /// An assembler holding nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: [0u8; MAX_TX_BYTES],
            len: 0,
            next_index: 0,
            in_progress: false,
        }
    }

    /// Whether a sequence is part-way through.
    #[must_use]
    pub const fn in_progress(&self) -> bool {
        self.in_progress
    }

    /// Bytes accumulated so far.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing has been accumulated.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Discards any sequence in progress.
    ///
    /// Zeroes the buffer rather than only resetting the length: the bytes are a
    /// transaction the user may have declined, and leaving them for the next
    /// command to partially overwrite is how a device signs a mixture of two.
    pub fn reset(&mut self) {
        self.buffer = [0u8; MAX_TX_BYTES];
        self.len = 0;
        self.next_index = 0;
        self.in_progress = false;
    }

    /// Feeds one command, returning the complete payload once it is complete.
    ///
    /// Returns `Ok(None)` while more chunks are expected.
    ///
    /// # Errors
    ///
    /// [`ApduError::NoSequenceInProgress`] for a continuation with nothing
    /// started, [`ApduError::OutOfOrder`] for a gap or a repeat, and
    /// [`ApduError::TooLarge`] past [`MAX_TX_BYTES`]. Any error leaves the
    /// assembler **empty**, not partially filled — a rejected sequence that
    /// stayed resumable is a sequence a host can splice.
    pub fn push(&mut self, command: &Command<'_>) -> Result<Option<&[u8]>, ApduError> {
        match command.chunk {
            Chunk::First => self.reset(),
            Chunk::More | Chunk::Last if !self.in_progress => {
                return Err(ApduError::NoSequenceInProgress);
            }
            Chunk::More | Chunk::Last => {}
        }

        let received = u16::from(command.p2);
        if received != self.next_index {
            let expected = self.next_index;
            self.reset();
            return Err(ApduError::OutOfOrder { expected, received });
        }

        let end = self.len + command.payload.len();
        if end > MAX_TX_BYTES {
            self.reset();
            return Err(ApduError::TooLarge {
                needed: end,
                limit: MAX_TX_BYTES,
            });
        }

        self.buffer[self.len..end].copy_from_slice(command.payload);
        self.len = end;
        self.next_index = self.next_index.wrapping_add(1);
        self.in_progress = true;

        if command.chunk == Chunk::Last {
            self.in_progress = false;
            Ok(Some(&self.buffer[..self.len]))
        } else {
            Ok(None)
        }
    }
}

/// Splits a response into APDU-sized pages.
///
/// A 11,165-byte signature is 44 responses. The iterator exists so the paging
/// is one tested thing rather than an index arithmetic bug repeated per command.
pub struct Pages<'a> {
    remaining: &'a [u8],
}

impl<'a> Pages<'a> {
    /// Pages `data` at [`MAX_APDU_PAYLOAD`].
    #[must_use]
    pub const fn new(data: &'a [u8]) -> Self {
        Self { remaining: data }
    }

    /// How many pages `len` bytes need.
    #[must_use]
    pub const fn count_for(len: usize) -> usize {
        len.div_ceil(MAX_APDU_PAYLOAD)
    }
}

impl<'a> Iterator for Pages<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        let take = self.remaining.len().min(MAX_APDU_PAYLOAD);
        let (page, rest) = self.remaining.split_at(take);
        self.remaining = rest;
        Some(page)
    }
}
