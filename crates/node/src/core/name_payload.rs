//! Wire forms of names with commit-reveal (ADR-135 P-3, feature bit 43).
//!
//! Four tags, 63-66, carried as one [`crate::core::TxKind::Name`]. Each form is
//! `tag ‖ fields`; labels are length-prefixed (one byte), so a form's size
//! depends on its label.
//!
//! Size per form, tag included: `CommitName` 33; `RegisterName` 70-130
//! (`1 + 1 + label + 32 + 1 + 32`, label 3-63); `RenewName` 6-66; `UpdateName`
//! 5 + label + argument. ADR-135's table reads 67-127 for `RegisterName`, which
//! is the size for a label of 0-60 bytes and contradicts its own 3-63 rule; this
//! file follows the rule and the spec records the corrected range.
//!
//! Stealth-address operations (`UpdateName` ops 0x02 and 0x05) need the ADR-102
//! meta-address encoding, which is not in this change; they are refused at decode.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// Tag of [`NameAction::Commit`].
pub const TAG_COMMIT_NAME: u8 = 63;
/// Tag of [`NameAction::Register`].
pub const TAG_REGISTER_NAME: u8 = 64;
/// Tag of [`NameAction::Renew`].
pub const TAG_RENEW_NAME: u8 = 65;
/// Tag of [`NameAction::Update`].
pub const TAG_UPDATE_NAME: u8 = 66;

/// Shortest label, in bytes (ADR-105; ASCII only).
pub const MIN_LABEL_LEN: usize = 3;
/// Longest label, in bytes.
pub const MAX_LABEL_LEN: usize = 63;

/// Wire op code of [`NameUpdate::SetTarget`].
pub const OP_SET_TARGET: u8 = 0x01;
/// Wire op code of [`NameUpdate::SetReverse`].
pub const OP_SET_REVERSE: u8 = 0x03;
/// Wire op code of [`NameUpdate::Transfer`].
pub const OP_TRANSFER: u8 = 0x04;

/// Checks a label: 3-63 bytes of `a-z`, `0-9` and `-`, no leading or trailing
/// `-`, no `--`. ASCII only, so no cross-script homoglyph can exist in a record
/// (ADR-105, threat model M-5).
///
/// # Errors
///
/// A static reason naming the first rule the label breaks.
pub fn validate_label(label: &[u8]) -> std::result::Result<(), &'static str> {
    if !(MIN_LABEL_LEN..=MAX_LABEL_LEN).contains(&label.len()) {
        return Err("a label is 3 to 63 bytes");
    }
    if !label
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    {
        return Err("a label is a-z, 0-9 and '-' only");
    }
    if label.first() == Some(&b'-') || label.last() == Some(&b'-') {
        return Err("a label does not start or end with '-'");
    }
    if label.windows(2).any(|w| w == b"--") {
        return Err("a label has no '--'");
    }
    Ok(())
}

/// The argument of [`NameAction::Update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameUpdate {
    /// Point the name at an address.
    SetTarget(Address),
    /// Make the sender's own address reverse-resolve to this name. Requires the
    /// name to target the sender at that moment, and anyone may call it.
    SetReverse,
    /// Give the name to a new owner.
    Transfer(Address),
}

/// One name operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameAction {
    /// Commit to a name: `commitment = name_commitment(label, sender, salt)`.
    /// Costs no fee (ADR-135 O-3).
    Commit {
        /// The 32-byte commitment.
        commitment: [u8; 32],
    },
    /// Reveal and register a name the sender committed to earlier.
    Register {
        /// The label, validated by [`validate_label`].
        label: Vec<u8>,
        /// The salt the commitment was made with.
        salt: [u8; 32],
        /// Years, `1 ..= 10`.
        periods: u8,
        /// The address the name points at.
        target: Address,
    },
    /// Extend a name's expiry by `periods` years.
    Renew {
        /// The label.
        label: Vec<u8>,
        /// Years, `1 ..= 10`.
        periods: u8,
    },
    /// Change a name the sender owns, or reverse-resolve it.
    Update {
        /// The label.
        label: Vec<u8>,
        /// The change.
        op: NameUpdate,
    },
}

fn put_label(buf: &mut Vec<u8>, label: &[u8]) {
    // Length-prefixed by one byte. A label past 255 bytes cannot be written
    // faithfully; it is never built by this crate (decode and apply refuse it).
    buf.push(u8::try_from(label.len()).unwrap_or(u8::MAX));
    buf.extend_from_slice(label);
}

impl NameAction {
    /// The form's wire tag.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Self::Commit { .. } => TAG_COMMIT_NAME,
            Self::Register { .. } => TAG_REGISTER_NAME,
            Self::Renew { .. } => TAG_RENEW_NAME,
            Self::Update { .. } => TAG_UPDATE_NAME,
        }
    }

    /// Short label for errors and logs.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Commit { .. } => "commit_name",
            Self::Register { .. } => "register_name",
            Self::Renew { .. } => "renew_name",
            Self::Update { .. } => "update_name",
        }
    }

    /// Appends `tag ‖ fields`.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.tag());
        match self {
            Self::Commit { commitment } => buf.extend_from_slice(commitment),
            Self::Register {
                label,
                salt,
                periods,
                target,
            } => {
                put_label(buf, label);
                buf.extend_from_slice(salt);
                buf.push(*periods);
                buf.extend_from_slice(target);
            }
            Self::Renew { label, periods } => {
                put_label(buf, label);
                buf.push(*periods);
            }
            Self::Update { label, op } => {
                put_label(buf, label);
                match op {
                    NameUpdate::SetTarget(address) => {
                        buf.push(OP_SET_TARGET);
                        buf.extend_from_slice(address);
                    }
                    NameUpdate::SetReverse => buf.push(OP_SET_REVERSE),
                    NameUpdate::Transfer(new_owner) => {
                        buf.push(OP_TRANSFER);
                        buf.extend_from_slice(new_owner);
                    }
                }
            }
        }
    }

    /// Decodes the fields after `tag`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a truncated section, a label that breaks
    /// [`validate_label`], an unknown or unsupported op, or a tag outside 63-66.
    pub fn decode(tag: u8, reader: &mut ByteReader<'_>) -> Result<Self> {
        match tag {
            TAG_COMMIT_NAME => Ok(Self::Commit {
                commitment: reader.read_array::<32>()?,
            }),
            TAG_REGISTER_NAME => {
                let label = read_label(reader)?;
                Ok(Self::Register {
                    label,
                    salt: reader.read_array::<32>()?,
                    periods: reader.read_u8()?,
                    target: reader.read_array::<32>()?,
                })
            }
            TAG_RENEW_NAME => {
                let label = read_label(reader)?;
                Ok(Self::Renew {
                    label,
                    periods: reader.read_u8()?,
                })
            }
            TAG_UPDATE_NAME => {
                let label = read_label(reader)?;
                let op = match reader.read_u8()? {
                    OP_SET_TARGET => NameUpdate::SetTarget(reader.read_array::<32>()?),
                    OP_SET_REVERSE => NameUpdate::SetReverse,
                    OP_TRANSFER => NameUpdate::Transfer(reader.read_array::<32>()?),
                    other => {
                        return Err(NodeError::Decode(format!(
                            "name update op {other} is not in this build"
                        )));
                    }
                };
                Ok(Self::Update { label, op })
            }
            other => Err(NodeError::Decode(format!("tag {other} is not a name form"))),
        }
    }
}

fn read_label(reader: &mut ByteReader<'_>) -> Result<Vec<u8>> {
    let len = usize::from(reader.read_u8()?);
    let label = reader.read_slice(len)?.to_vec();
    validate_label(&label).map_err(|why| NodeError::Decode(format!("name label: {why}")))?;
    Ok(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_must_be_three_to_sixty_three_ascii_bytes_without_edge_or_double_dashes() {
        assert!(validate_label(b"abc").is_ok());
        assert!(validate_label(&[b'a'; 63]).is_ok());
        assert!(validate_label(&[b'a'; 64]).is_err());
        assert!(validate_label(b"ab").is_err());
        assert!(validate_label(b"-ab").is_err());
        assert!(validate_label(b"ab-").is_err());
        assert!(validate_label(b"a--b").is_err());
        assert!(validate_label(b"aBc").is_err());
    }

    #[test]
    fn every_form_round_trips() {
        let kinds = [
            NameAction::Commit {
                commitment: [1; 32],
            },
            NameAction::Register {
                label: b"alice".to_vec(),
                salt: [2; 32],
                periods: 3,
                target: [3; 32],
            },
            NameAction::Renew {
                label: b"alice".to_vec(),
                periods: 1,
            },
            NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::SetTarget([4; 32]),
            },
            NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::SetReverse,
            },
            NameAction::Update {
                label: b"alice".to_vec(),
                op: NameUpdate::Transfer([5; 32]),
            },
        ];
        for kind in kinds {
            let mut buf = Vec::new();
            kind.encode_into(&mut buf);
            let mut reader = ByteReader::new(&buf);
            let tag = reader.read_u8().unwrap();
            let decoded = NameAction::decode(tag, &mut reader).unwrap();
            reader.finish().unwrap();
            assert_eq!(decoded, kind);
        }
    }

    #[test]
    fn a_stealth_op_is_refused_at_decode() {
        let mut buf = vec![TAG_UPDATE_NAME, 3];
        buf.extend_from_slice(b"abc");
        buf.push(0x02);
        let mut reader = ByteReader::new(&buf[1..]);
        assert!(NameAction::decode(TAG_UPDATE_NAME, &mut reader).is_err());
    }
}
