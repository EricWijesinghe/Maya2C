//! What the user approves: a v7 (suite-tagged) transfer, parsed from the bytes
//! the device is about to sign.
//!
//! # The device signs only what it can show
//!
//! The host sends the transaction's **signing bytes** (`suite_tx::signing_bytes`
//! in the node), and the device signs exactly those. So before a user is asked
//! anything, the bytes are parsed here and refused unless they are:
//!
//! - under the v7 signing domain, so they are a transaction and not some
//!   other message a host would like signed;
//! - for suite `0x10` with **this device's** public key, so a host cannot get
//!   a signature for a key the user did not choose, and the address on screen
//!   is the account that pays;
//! - a plain transfer with at most [`MAX_OUTPUTS`] outputs, all of which fit
//!   on the screen. A payload kind (a swap, a governance vote, …) is refused,
//!   not displayed as a transfer: the device cannot describe what it does.
//!
//! Everything here is `no_std`, allocation-free, and tested on the host.

use crate::suite::{PUBLIC_KEY_LEN, SUITE_ID};

/// The node's v7 signing domain (`crates/node/src/core/suite_tx.rs`).
pub const TX_DOMAIN_SUITE: &[u8] = b"custom-l1-node.tx.suite.v1";
/// Most outputs a reviewed transfer may have.
pub const MAX_OUTPUTS: usize = 4;
/// Most inputs a reviewed transfer may spend.
pub const MAX_INPUTS: usize = 32;

const INPUT_BYTES: usize = 36;
const OUTPUT_BYTES: usize = 40;

/// One output, as the screen shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// Base units.
    pub amount: u64,
    /// Recipient address.
    pub recipient: [u8; 32],
}

/// A transfer the user can approve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Review {
    /// The outputs; only the first `output_count` are meaningful.
    pub outputs: [Output; MAX_OUTPUTS],
    /// How many outputs there are.
    pub output_count: usize,
    /// How many inputs it spends.
    pub input_count: usize,
    /// The sender's nonce.
    pub nonce: u64,
}

/// Why the device will not show, and so will not sign, these bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewError {
    /// Not under the v7 signing domain.
    NotATransaction,
    /// Shorter than its own counts say.
    Truncated,
    /// More inputs or outputs than the device will show, or no outputs.
    TooMany,
    /// A suite other than `0x10`.
    WrongSuite(u8),
    /// A public key other than this device's.
    ForeignKey,
    /// Anything after the nonce: a payload kind the device cannot describe.
    NotATransfer,
}

impl ReviewError {
    /// The status word a host sees: "incorrect data", distinct per cause in
    /// the low byte so a wallet can say which.
    #[must_use]
    pub const fn status_word(self) -> u16 {
        match self {
            Self::NotATransaction => 0x6A90,
            Self::Truncated => 0x6A91,
            Self::TooMany => 0x6A92,
            Self::WrongSuite(_) => 0x6A93,
            Self::ForeignKey => 0x6A94,
            Self::NotATransfer => 0x6A95,
        }
    }
}

/// A cursor that refuses to read past the end.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ReviewError> {
        if self.0.len() < n {
            return Err(ReviewError::Truncated);
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn u64(&mut self) -> Result<u64, ReviewError> {
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, ReviewError> {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(bytes))
    }

    /// A count, bounded before anything it counts is read.
    fn count(&mut self, max: usize) -> Result<usize, ReviewError> {
        let count = self.u64()?;
        usize::try_from(count)
            .ok()
            .filter(|&c| c <= max)
            .ok_or(ReviewError::TooMany)
    }
}

/// Parses `signing_bytes` for review against `device_key`.
///
/// # Errors
///
/// A [`ReviewError`] for anything the device will not put on screen.
pub fn review(
    signing_bytes: &[u8],
    device_key: &[u8; PUBLIC_KEY_LEN],
) -> Result<Review, ReviewError> {
    let mut r = Reader(signing_bytes);
    if r.take(TX_DOMAIN_SUITE.len()).ok() != Some(TX_DOMAIN_SUITE) {
        return Err(ReviewError::NotATransaction);
    }

    let input_count = r.count(MAX_INPUTS)?;
    r.take(input_count * INPUT_BYTES)?;

    let output_count = r.count(MAX_OUTPUTS)?;
    if output_count == 0 {
        return Err(ReviewError::TooMany);
    }
    let mut outputs = [Output::default(); MAX_OUTPUTS];
    for output in outputs.iter_mut().take(output_count) {
        let raw = r.take(OUTPUT_BYTES)?;
        let mut amount = [0u8; 8];
        amount.copy_from_slice(&raw[..8]);
        output.amount = u64::from_le_bytes(amount);
        output.recipient.copy_from_slice(&raw[8..]);
    }

    let suite = r.take(1)?[0];
    if suite != SUITE_ID {
        return Err(ReviewError::WrongSuite(suite));
    }
    if usize::try_from(r.u32()?).ok() != Some(PUBLIC_KEY_LEN) {
        return Err(ReviewError::ForeignKey);
    }
    if r.take(PUBLIC_KEY_LEN)? != device_key.as_slice() {
        return Err(ReviewError::ForeignKey);
    }
    let nonce = r.u64()?;
    if !r.0.is_empty() {
        return Err(ReviewError::NotATransfer);
    }
    Ok(Review {
        outputs,
        output_count,
        input_count,
        nonce,
    })
}
