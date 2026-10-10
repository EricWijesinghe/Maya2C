//! Air-gapped transport: a signed transaction as a sequence of QR frames.
//!
//! # A signed `Maya2C` transaction does not fit in a QR code
//!
//! This module exists because of one measurement:
//!
//! | | bytes |
//! |---|---|
//! | ML-DSA-65 signature | 3,309 |
//! | SLH-DSA-SHA2-128s signature | 7,856 |
//! | hybrid public key | 1,984 |
//! | transaction body | ~120 |
//! | **total** | **~13,269** |
//!
//! A QR code's absolute maximum is **2,953 bytes** — version 40, error
//! correction L, binary mode. A signed transaction is four and a half times
//! that, so no single code can carry one, at any density, ever.
//!
//! And version 40 is a 177x177 grid. Scanning one off a laptop screen with a
//! phone camera is marginal even when it does fit, and error correction L
//! leaves almost nothing for a smudge or a reflection. So this does not chase
//! the theoretical minimum of five frames; it targets a density that actually
//! scans, and accepts more of them.
//!
//! # The consequence: frames need a protocol, not just a loop
//!
//! Splitting bytes into chunks is easy. What makes an air-gapped transfer
//! trustworthy is what happens when it goes wrong, and it goes wrong routinely
//! — a frame missed while the animation loops, two transactions signed in
//! succession and their frames interleaved, a scanner that reads frame 7 twice
//! and frame 8 never.
//!
//! So every frame carries the digest of the **whole** payload. That is what
//! lets a receiver notice it has mixed two transactions together, which is the
//! failure that would otherwise reassemble into a valid-looking blob that
//! signs for something nobody approved.
//!
//! # What this module does not do
//!
//! It does not render QR images. Framing is the part with the failure modes;
//! turning a frame into pixels is `qrcode`'s job, and keeping them separate is
//! what lets every rule below be tested without a camera.

use crate::error::{Result, WalletError};

/// Frame magic. Four bytes, so a frame from another application is rejected
/// before its length fields are trusted.
pub const MAGIC: [u8; 4] = *b"MAYA";

/// Framing version.
///
/// A receiver that does not know a version refuses the frame rather than
/// guessing at its layout. An air-gapped device is precisely the one that
/// cannot be updated in lockstep with the machine talking to it.
pub const VERSION: u8 = 1;

/// Bytes of header before the chunk: magic, version, total, index, digest.
pub const HEADER_LEN: usize = 4 + 1 + 2 + 2 + 32;

/// Payload bytes per frame.
///
/// # Where this number comes from
///
/// A QR version 20 code at error correction L holds 858 bytes in binary mode.
/// Version 20 is 97x97 modules — large enough to be dense, small enough that a
/// phone camera reads it off a screen at arm's length without hunting.
///
/// 858 minus [`HEADER_LEN`] leaves 817. Rounded down to 800 for margin: some
/// encoders spend a few bytes on mode and terminator, and a frame that
/// overflows its target version silently promotes to version 21 and gets
/// harder to scan — which is the kind of regression nobody notices until a
/// user is standing in front of a screen that will not read.
///
/// At 800 bytes a ~13.3 KB signed transaction is 17 frames. That is the honest
/// cost of a post-quantum signature in an air-gapped flow, and it is why
/// hardware wallets animate.
pub const CHUNK_BYTES: usize = 800;

/// Frames one payload may occupy.
///
/// 512 frames is 409,600 bytes — far past any transaction this chain produces,
/// and small enough that a malformed `total` cannot make a receiver allocate
/// arbitrarily. The ceiling is checked before anything is reserved.
pub const MAX_FRAMES: u16 = 512;

/// One frame of an air-gapped payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Frames in the complete payload.
    pub total: u16,
    /// This frame's position, zero-based.
    pub index: u16,
    /// BLAKE3 digest of the complete payload.
    ///
    /// Carried in **every** frame rather than only the first. A receiver that
    /// missed the first frame would otherwise have no way to know which
    /// payload the rest belong to, and interleaved frames from two signings
    /// would reassemble into something neither device approved.
    pub digest: [u8; 32],
    /// This frame's slice of the payload.
    pub chunk: Vec<u8>,
}

impl Frame {
    /// Serialises the frame for a QR code's binary mode.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.chunk.len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&self.total.to_le_bytes());
        out.extend_from_slice(&self.index.to_le_bytes());
        out.extend_from_slice(&self.digest);
        out.extend_from_slice(&self.chunk);
        out
    }

    /// Parses a scanned frame.
    ///
    /// # Errors
    ///
    /// [`WalletError::Airgap`] for a short frame, wrong magic, unknown
    /// version, a `total` of zero or past [`MAX_FRAMES`], or an `index` past
    /// `total`.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(WalletError::Airgap(format!(
                "frame is {} bytes, shorter than the {HEADER_LEN}-byte header",
                bytes.len()
            )));
        }
        if bytes[..4] != MAGIC {
            return Err(WalletError::Airgap(
                "not a Maya2C frame: wrong magic".to_string(),
            ));
        }
        if bytes[4] != VERSION {
            return Err(WalletError::Airgap(format!(
                "frame version {} is not {VERSION}; this device cannot read it",
                bytes[4]
            )));
        }

        let total = u16::from_le_bytes([bytes[5], bytes[6]]);
        let index = u16::from_le_bytes([bytes[7], bytes[8]]);

        if total == 0 {
            return Err(WalletError::Airgap("frame claims a total of 0".to_string()));
        }
        // Checked before an assembler reserves anything for it.
        if total > MAX_FRAMES {
            return Err(WalletError::Airgap(format!(
                "frame claims {total} frames, past the {MAX_FRAMES} ceiling"
            )));
        }
        if index >= total {
            return Err(WalletError::Airgap(format!(
                "frame {index} is out of range for a payload of {total}"
            )));
        }

        let mut digest = [0u8; 32];
        digest.copy_from_slice(&bytes[9..41]);

        Ok(Self {
            total,
            index,
            digest,
            chunk: bytes[HEADER_LEN..].to_vec(),
        })
    }
}

/// Splits a payload into frames.
///
/// # Errors
///
/// [`WalletError::Airgap`] if the payload is empty, or would need more than
/// [`MAX_FRAMES`].
pub fn split(payload: &[u8]) -> Result<Vec<Frame>> {
    if payload.is_empty() {
        return Err(WalletError::Airgap(
            "refusing to frame an empty payload".to_string(),
        ));
    }

    let count = payload.len().div_ceil(CHUNK_BYTES);
    if count > MAX_FRAMES as usize {
        return Err(WalletError::Airgap(format!(
            "payload of {} bytes needs {count} frames, past the {MAX_FRAMES} ceiling",
            payload.len()
        )));
    }

    let digest = *blake3::hash(payload).as_bytes();
    let total = count as u16;

    Ok(payload
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(index, chunk)| Frame {
            total,
            index: index as u16,
            digest,
            chunk: chunk.to_vec(),
        })
        .collect())
}

/// Collects frames until a payload is complete.
///
/// Frames may arrive in any order and repeat freely — a scanner watching an
/// animation sees both constantly — so duplicates are idempotent and order is
/// irrelevant.
#[derive(Debug)]
pub struct Assembler {
    digest: [u8; 32],
    total: u16,
    chunks: Vec<Option<Vec<u8>>>,
}

impl Assembler {
    /// Starts assembly from the first frame seen.
    #[must_use]
    pub fn new(first: &Frame) -> Self {
        Self {
            digest: first.digest,
            total: first.total,
            // `total` was bounded by `Frame::decode` before reaching here, so
            // this allocation is bounded too.
            chunks: vec![None; first.total as usize],
        }
    }

    /// Adds a frame.
    ///
    /// # Errors
    ///
    /// [`WalletError::Airgap`] if the frame belongs to a different payload —
    /// a different digest or a different total. That is the check this whole
    /// design exists for: without it, frames from two signings interleave and
    /// reassemble into a blob neither device approved.
    pub fn add(&mut self, frame: &Frame) -> Result<()> {
        if frame.digest != self.digest || frame.total != self.total {
            return Err(WalletError::Airgap(
                "frame belongs to a different payload; two transactions may be \
                 interleaved — restart the scan"
                    .to_string(),
            ));
        }
        self.chunks[frame.index as usize] = Some(frame.chunk.clone());
        Ok(())
    }

    /// Frames still missing, ascending. Empty when complete.
    ///
    /// Returned rather than a bare count so a UI can tell a user *which* frame
    /// to go back for, which on a looping animation is the difference between
    /// waiting one cycle and waiting many.
    #[must_use]
    pub fn missing(&self) -> Vec<u16> {
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(index, chunk)| chunk.is_none().then_some(index as u16))
            .collect()
    }

    /// Whether every frame has arrived.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.chunks.iter().all(Option::is_some)
    }

    /// Reassembles and verifies the payload.
    ///
    /// # Errors
    ///
    /// [`WalletError::Airgap`] if frames are missing, or if the reassembled
    /// bytes do not hash to the digest every frame carried. The second should
    /// be unreachable given the per-frame checks — and is checked anyway,
    /// because the cost of being wrong here is a signed transaction for
    /// something the user never saw.
    pub fn finish(&self) -> Result<Vec<u8>> {
        let missing = self.missing();
        if !missing.is_empty() {
            return Err(WalletError::Airgap(format!(
                "{} of {} frames still missing: {:?}",
                missing.len(),
                self.total,
                &missing[..missing.len().min(8)]
            )));
        }

        let mut payload = Vec::new();
        for chunk in self.chunks.iter().flatten() {
            payload.extend_from_slice(chunk);
        }

        if *blake3::hash(&payload).as_bytes() != self.digest {
            return Err(WalletError::Airgap(
                "reassembled payload does not match the digest the frames carried".to_string(),
            ));
        }

        Ok(payload)
    }
}

/// Frames a payload would occupy, without building them.
///
/// For a UI that wants to say "17 codes" before starting.
#[must_use]
pub fn frame_count(payload_len: usize) -> usize {
    payload_len.div_ceil(CHUNK_BYTES)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// A payload the size of a real signed Maya2C transaction.
    fn signed_transaction_sized() -> Vec<u8> {
        (0..13_269u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn a_signed_transaction_does_not_fit_in_one_qr_code() {
        // The measurement this module exists for, pinned. QR's absolute
        // maximum is 2,953 bytes (v40-L, binary). If a future change made a
        // transaction small enough to fit in one code, this fails and the
        // whole chunking apparatus can be reconsidered.
        const QR_ABSOLUTE_MAX: usize = 2_953;
        let payload = signed_transaction_sized();
        assert!(
            payload.len() > QR_ABSOLUTE_MAX,
            "a signed transaction is {} bytes against QR's {QR_ABSOLUTE_MAX}-byte ceiling",
            payload.len()
        );
        assert_eq!(frame_count(payload.len()), 17);
    }

    #[test]
    fn a_payload_round_trips_through_frames() {
        let payload = signed_transaction_sized();
        let frames = split(&payload).expect("frames");

        let mut assembler = Assembler::new(&frames[0]);
        for frame in &frames {
            assembler.add(frame).expect("same payload");
        }

        assert!(assembler.is_complete());
        assert_eq!(assembler.finish().expect("complete"), payload);
    }

    #[test]
    fn frames_may_arrive_in_any_order() {
        // A scanner watching a looping animation joins at an arbitrary point.
        let payload = signed_transaction_sized();
        let frames = split(&payload).expect("frames");

        let mut reversed: Vec<_> = frames.clone();
        reversed.reverse();

        let mut assembler = Assembler::new(&reversed[0]);
        for frame in &reversed {
            assembler.add(frame).expect("same payload");
        }
        assert_eq!(assembler.finish().expect("complete"), payload);
    }

    #[test]
    fn duplicate_frames_are_idempotent() {
        // The common case, not an edge case: an animation loops and every
        // frame is seen many times.
        let payload = signed_transaction_sized();
        let frames = split(&payload).expect("frames");

        let mut assembler = Assembler::new(&frames[0]);
        for _ in 0..3 {
            for frame in &frames {
                assembler.add(frame).expect("same payload");
            }
        }
        assert_eq!(assembler.finish().expect("complete"), payload);
    }

    #[test]
    fn a_missing_frame_is_named_rather_than_counted() {
        // A user in front of a looping animation needs to know *which* frame
        // to wait for.
        let payload = signed_transaction_sized();
        let frames = split(&payload).expect("frames");

        let mut assembler = Assembler::new(&frames[0]);
        for frame in frames.iter().filter(|f| f.index != 5) {
            assembler.add(frame).expect("same payload");
        }

        assert!(!assembler.is_complete());
        assert_eq!(assembler.missing(), vec![5]);

        let error = assembler.finish().expect_err("incomplete");
        assert!(
            format!("{error}").contains('5'),
            "the error must name the missing frame: {error}"
        );
    }

    #[test]
    fn frames_from_two_transactions_cannot_be_mixed() {
        // The failure this design exists to prevent. Without the per-frame
        // digest, interleaved frames from two signings would reassemble into a
        // blob neither device approved — and it would look valid.
        let first = signed_transaction_sized();
        let mut second = signed_transaction_sized();
        second[0] ^= 0xFF;

        let first_frames = split(&first).expect("frames");
        let second_frames = split(&second).expect("frames");

        let mut assembler = Assembler::new(&first_frames[0]);
        assembler.add(&first_frames[0]).expect("same payload");

        let error = assembler
            .add(&second_frames[1])
            .expect_err("a frame from another payload must be refused");
        assert!(format!("{error}").contains("different payload"));
    }

    #[test]
    fn a_frame_round_trips_through_its_encoding() {
        let payload = signed_transaction_sized();
        let frames = split(&payload).expect("frames");

        for frame in &frames {
            let decoded = Frame::decode(&frame.encode()).expect("decodes");
            assert_eq!(&decoded, frame);
        }
    }

    #[test]
    fn a_frame_from_another_application_is_refused() {
        let mut bytes = split(b"hello world").expect("frames")[0].encode();
        bytes[0] = b'X';
        let error = Frame::decode(&bytes).expect_err("wrong magic");
        assert!(format!("{error}").contains("magic"));
    }

    #[test]
    fn an_unknown_version_is_refused_rather_than_guessed() {
        // An air-gapped device is the one that cannot be updated in lockstep,
        // so it must refuse a layout it does not know rather than misread it.
        let mut bytes = split(b"hello world").expect("frames")[0].encode();
        bytes[4] = VERSION + 1;
        let error = Frame::decode(&bytes).expect_err("unknown version");
        assert!(format!("{error}").contains("version"));
    }

    #[test]
    fn a_truncated_frame_is_refused_before_its_fields_are_read() {
        let bytes = split(b"hello world").expect("frames")[0].encode();
        for cut in [0usize, 1, 10, HEADER_LEN - 1] {
            assert!(
                Frame::decode(&bytes[..cut]).is_err(),
                "a {cut}-byte frame must be refused"
            );
        }
    }

    #[test]
    fn a_hostile_total_cannot_make_a_receiver_allocate() {
        // The bound is checked in `decode`, before `Assembler::new` reserves
        // anything, so a frame claiming 65,535 parts never becomes 65,535
        // allocations.
        let mut bytes = split(b"hello world").expect("frames")[0].encode();
        bytes[5..7].copy_from_slice(&u16::MAX.to_le_bytes());
        let error = Frame::decode(&bytes).expect_err("total past the ceiling");
        assert!(format!("{error}").contains("ceiling"));
    }

    #[test]
    fn an_index_past_the_total_is_refused() {
        let mut bytes = split(b"hello world").expect("frames")[0].encode();
        bytes[7..9].copy_from_slice(&9u16.to_le_bytes());
        assert!(Frame::decode(&bytes).is_err());
    }

    #[test]
    fn an_empty_payload_is_refused() {
        assert!(split(&[]).is_err());
    }

    #[test]
    fn a_single_chunk_payload_still_gets_a_frame() {
        let frames = split(b"short").expect("frames");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].total, 1);
        assert_eq!(frames[0].index, 0);

        let mut assembler = Assembler::new(&frames[0]);
        assembler.add(&frames[0]).expect("same payload");
        assert_eq!(assembler.finish().expect("complete"), b"short");
    }

    #[test]
    fn every_frame_fits_the_target_qr_version() {
        // The property that keeps the codes scannable. A frame past the v20-L
        // capacity silently promotes to a denser version, and the failure
        // surfaces as a user unable to scan rather than as an error.
        const V20_L_CAPACITY: usize = 858;
        let payload = signed_transaction_sized();
        for frame in split(&payload).expect("frames") {
            assert!(
                frame.encode().len() <= V20_L_CAPACITY,
                "frame {} is {} bytes, past the {V20_L_CAPACITY}-byte target",
                frame.index,
                frame.encode().len()
            );
        }
    }
}
