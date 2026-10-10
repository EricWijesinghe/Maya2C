//! The wire format, and nothing else.
//!
//! Framing is separated from the transport that carries it because the two have
//! different lifetimes. A ceremony run over mutual TLS ([`crate::tls`]) and a
//! ceremony run by walking encrypted USB sticks between three data centres are
//! the same protocol; only one of them has a socket. An institution that
//! air-gaps its custodians should not have to reimplement the encoding to do it.
//!
//! # Every length is bounded before it is allocated
//!
//! Each variable-length field is preceded by its length, and every length is
//! checked against a compiled-in maximum *before* a buffer is reserved. A
//! length prefix read from a socket is an attacker-controlled allocation
//! request, and "the peer was authenticated" is not an argument — the peer
//! being authenticated is what gets it close enough to send one.

use maya_crypto_pq::kem::ENCAPSULATION_KEY_LEN;

use crate::dkg::{Announcement, Dealing, VaultId};
use crate::error::{CustodyError, Result};
use crate::seal::{SEALED_SHARE_LEN, SealedShare};
use crate::session::SigningRequest;
use crate::vss::Commitments;

/// The largest frame this codec will emit or accept.
///
/// A dealing for the maximum roster is the biggest legitimate message: 254
/// sealed shares of 1,153 bytes each, plus commitments. 512 KiB clears that
/// with room and refuses anything an order of magnitude past it.
pub const MAX_FRAME_LEN: usize = 512 * 1024;

/// The largest message a vault will be asked to sign.
///
/// A `Maya2C` transaction is kilobytes, not megabytes. The bound exists so that a
/// signing request cannot be used to make every custodian in a vault allocate
/// at once.
pub const MAX_SIGNED_MESSAGE_LEN: usize = 64 * 1024;

/// One protocol message.
#[derive(Clone, Debug)]
pub enum Frame {
    /// Round one of a ceremony.
    ///
    /// Boxed like the others: an announcement carries a 1,184-byte ML-KEM
    /// encapsulation key, and an enum whose smallest variant costs as much as
    /// its largest is one every caller pays for.
    Announce(Box<Announcement>),
    /// Round two of a ceremony.
    Deal(Box<Dealing>),
    /// A combiner asking a custodian to contribute.
    Request(Box<SigningRequest>),
    /// A custodian's sealed contribution.
    Contribution(SealedShare),
}

const TAG_ANNOUNCE: u8 = 0x01;
const TAG_DEAL: u8 = 0x02;
const TAG_REQUEST: u8 = 0x03;
const TAG_CONTRIBUTION: u8 = 0x04;

impl Frame {
    /// Encodes the frame, without a length prefix.
    ///
    /// The prefix belongs to whatever is delimiting messages on the wire —
    /// [`crate::tls`] writes one; a file on a USB stick does not need one.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Announce(announcement) => {
                out.push(TAG_ANNOUNCE);
                out.push(announcement.index);
                out.extend_from_slice(&announcement.encapsulation_key);
            }
            Self::Deal(dealing) => {
                out.push(TAG_DEAL);
                out.push(dealing.dealer);
                push_u16(&mut out, dealing.commitments.len());
                for point in &dealing.commitments.0 {
                    out.extend_from_slice(point.as_bytes());
                }
                push_u16(&mut out, dealing.sealed.len());
                for sealed in &dealing.sealed {
                    push_sealed(&mut out, sealed);
                }
            }
            Self::Request(request) => {
                out.push(TAG_REQUEST);
                out.extend_from_slice(&request.vault.0);
                out.extend_from_slice(&request.encapsulation_key);
                push_u32(&mut out, request.message.len());
                out.extend_from_slice(&request.message);
            }
            Self::Contribution(sealed) => {
                out.push(TAG_CONTRIBUTION);
                push_sealed(&mut out, sealed);
            }
        }
        out
    }

    /// Decodes a frame.
    ///
    /// # Errors
    ///
    /// [`CustodyError::Malformed`] for an unknown tag, a truncated field, a
    /// length past its bound, or trailing bytes. Trailing bytes are an error
    /// rather than ignored: a decoder that tolerates them accepts two byte
    /// strings for one message, and this protocol hashes its messages.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_FRAME_LEN {
            return Err(CustodyError::Malformed("frame over the size bound"));
        }
        let mut reader = Reader::new(bytes);
        let frame = match reader.byte()? {
            TAG_ANNOUNCE => {
                let index = reader.byte()?;
                let mut encapsulation_key = [0u8; ENCAPSULATION_KEY_LEN];
                encapsulation_key.copy_from_slice(reader.take(ENCAPSULATION_KEY_LEN)?);
                Self::Announce(Box::new(Announcement {
                    index,
                    encapsulation_key,
                }))
            }
            TAG_DEAL => {
                let dealer = reader.byte()?;
                let count = reader.u16()?;
                // Reserve for what the remaining bytes could hold, not for
                // what the count claims: a ten-byte frame claiming 65,535
                // points would otherwise reserve 2 MiB before failing.
                let mut points = Vec::with_capacity(count.min(reader.remaining() / 32));
                for _ in 0..count {
                    let mut point = [0u8; 32];
                    point.copy_from_slice(reader.take(32)?);
                    points.push(curve25519_dalek::ristretto::CompressedRistretto(point));
                }
                let sealed_count = reader.u16()?;
                let mut sealed = Vec::with_capacity(
                    sealed_count.min(reader.remaining() / (SEALED_SHARE_LEN + 6)),
                );
                for _ in 0..sealed_count {
                    sealed.push(reader.sealed()?);
                }
                Self::Deal(Box::new(Dealing {
                    dealer,
                    commitments: Commitments(points),
                    sealed,
                }))
            }
            TAG_REQUEST => {
                let mut vault = [0u8; 32];
                vault.copy_from_slice(reader.take(32)?);
                let mut encapsulation_key = [0u8; ENCAPSULATION_KEY_LEN];
                encapsulation_key.copy_from_slice(reader.take(ENCAPSULATION_KEY_LEN)?);
                let length = reader.u32()?;
                if length > MAX_SIGNED_MESSAGE_LEN {
                    return Err(CustodyError::Malformed("signed message over the bound"));
                }
                let message = reader.take(length)?.to_vec();
                Self::Request(Box::new(SigningRequest {
                    vault: VaultId(vault),
                    message,
                    encapsulation_key,
                }))
            }
            TAG_CONTRIBUTION => Self::Contribution(reader.sealed()?),
            _ => return Err(CustodyError::Malformed("unknown frame tag")),
        };

        if reader.remaining() != 0 {
            return Err(CustodyError::Malformed("trailing bytes after frame"));
        }
        Ok(frame)
    }
}

fn push_u16(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u16).to_be_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: usize) {
    out.extend_from_slice(&(value as u32).to_be_bytes());
}

fn push_sealed(out: &mut Vec<u8>, sealed: &SealedShare) {
    out.push(sealed.dealer);
    out.push(sealed.recipient);
    push_u32(out, sealed.body.len());
    out.extend_from_slice(&sealed.body);
}

/// A cursor that refuses to read past its end.
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(CustodyError::Malformed("length overflows"))?;
        if end > self.bytes.len() {
            return Err(CustodyError::Malformed("frame truncated"));
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<usize> {
        let bytes = self.take(2)?;
        Ok(usize::from(u16::from_be_bytes([bytes[0], bytes[1]])))
    }

    fn u32(&mut self) -> Result<usize> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    }

    /// Reads a sealed share, checking its length against the one length a
    /// sealed share can have.
    ///
    /// Fixed rather than bounded: a sealed share is an ML-KEM ciphertext and an
    /// AEAD ciphertext over a fixed-size plaintext, so any other length is a
    /// malformed message and not a variant.
    fn sealed(&mut self) -> Result<SealedShare> {
        let dealer = self.byte()?;
        let recipient = self.byte()?;
        let length = self.u32()?;
        if length != SEALED_SHARE_LEN {
            return Err(CustodyError::Malformed("sealed share of the wrong length"));
        }
        Ok(SealedShare {
            dealer,
            recipient,
            body: self.take(length)?.to_vec(),
        })
    }
}
