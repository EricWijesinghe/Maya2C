//! Wire forms of scheduled payments (ADR-135 P-7, bit 46).
//!
//! Two tags, 67 and 68, carried as one [`crate::core::TxKind::Schedule`] so
//! each exhaustive match over kinds gains one arm. `tag ‖ fields`, fixed width,
//! exactly as ADR-135 lays them out; the tag is written here and read by
//! `TxKind::decode`. `TopUpSchedule` is cut (ADR-135 C-4).

use primitive_types::U256;

use crate::core::codec::{ByteReader, put_amount};
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// Tag of [`ScheduleAction::Create`].
pub const TAG_CREATE_SCHEDULE: u8 = 67;
/// Tag of [`ScheduleAction::Cancel`].
pub const TAG_CANCEL_SCHEDULE: u8 = 68;

/// One scheduled-payment operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleAction {
    /// Prepay `runs` payments of `amount` to `payee`, the first at
    /// `first_due`, then every `interval` blocks.
    Create {
        /// Account each run credits.
        payee: Address,
        /// Native units per run.
        amount: U256,
        /// Height of the first run; above the creating block's height.
        first_due: u64,
        /// Blocks between runs.
        interval: u64,
        /// Number of runs, 1 to `MAX_RUNS`.
        runs: u32,
    },
    /// Close a schedule and return its unspent escrow. Payer only.
    Cancel {
        /// The schedule to close.
        schedule_id: [u8; 32],
    },
}

impl ScheduleAction {
    /// The form's wire tag.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Self::Create { .. } => TAG_CREATE_SCHEDULE,
            Self::Cancel { .. } => TAG_CANCEL_SCHEDULE,
        }
    }

    /// Short label for errors and logs.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Create { .. } => "create_schedule",
            Self::Cancel { .. } => "cancel_schedule",
        }
    }

    /// Appends `tag ‖ fields`.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.tag());
        match self {
            Self::Create {
                payee,
                amount,
                first_due,
                interval,
                runs,
            } => {
                buf.extend_from_slice(payee);
                put_amount(buf, amount);
                buf.extend_from_slice(&first_due.to_le_bytes());
                buf.extend_from_slice(&interval.to_le_bytes());
                buf.extend_from_slice(&runs.to_le_bytes());
            }
            Self::Cancel { schedule_id } => buf.extend_from_slice(schedule_id),
        }
    }

    /// Decodes the fields after `tag`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a truncated section or a tag other than 67/68.
    pub fn decode(tag: u8, reader: &mut ByteReader<'_>) -> Result<Self> {
        match tag {
            TAG_CREATE_SCHEDULE => Ok(Self::Create {
                payee: reader.read_array::<32>()?,
                amount: reader.read_amount()?,
                first_due: reader.read_u64()?,
                interval: reader.read_u64()?,
                runs: reader.read_u32()?,
            }),
            TAG_CANCEL_SCHEDULE => Ok(Self::Cancel {
                schedule_id: reader.read_array::<32>()?,
            }),
            other => Err(NodeError::Decode(format!(
                "tag {other} is not a schedule form"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn round_trip(action: ScheduleAction, size: usize) {
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        assert_eq!(buf.len(), size, "{}", action.label());
        let mut reader = ByteReader::new(&buf[1..]);
        assert_eq!(ScheduleAction::decode(buf[0], &mut reader).unwrap(), action);
        reader.finish().unwrap();
    }

    #[test]
    fn both_forms_have_the_adr_135_size_and_round_trip() {
        round_trip(
            ScheduleAction::Create {
                payee: [2; 32],
                amount: U256::MAX,
                first_due: 11,
                interval: 3_600,
                runs: 120,
            },
            85,
        );
        round_trip(
            ScheduleAction::Cancel {
                schedule_id: [3; 32],
            },
            33,
        );
    }

    #[test]
    fn a_truncated_form_is_a_decode_error() {
        let mut reader = ByteReader::new(&[0u8; 83]);
        assert!(ScheduleAction::decode(TAG_CREATE_SCHEDULE, &mut reader).is_err());
    }
}
