//! Vault actions (ADR-030; Master Prompts 22 and 28).
//!
//! ```text
//! action = 0 delay:u64 limit:u64 n:u64 guardian[32]*n    configure
//!        | 1 to[32] amount:u64                           request
//!        | 2 owner[32] id:u64                            execute
//!        | 3 owner[32] id:u64                            cancel
//! ```
//!
//! The sender is always the account acting: the vault for `configure` and
//! `request`, anyone for `execute`, a guardian or the owner for `cancel`.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Most guardians a vault may name.
pub const MAX_GUARDIANS: usize = 16;

/// The id [`VaultAction::Cancel`] uses for a queued reconfiguration.
pub const RECONFIGURE_ID: u64 = u64::MAX;

const CONFIGURE: u8 = 0;
const REQUEST: u8 = 1;
const EXECUTE: u8 = 2;
const CANCEL: u8 = 3;

/// A vault's policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultConfig {
    /// Blocks a withdrawal (or a reconfiguration) waits.
    pub delay_blocks: u64,
    /// The most a plain transfer may move out at once, fees aside.
    pub limit: u64,
    /// Accounts that may cancel.
    pub guardians: Vec<[u8; 32]>,
}

impl VaultConfig {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.delay_blocks.to_le_bytes());
        buf.extend_from_slice(&self.limit.to_le_bytes());
        buf.extend_from_slice(&(self.guardians.len() as u64).to_le_bytes());
        for g in &self.guardians {
            buf.extend_from_slice(g);
        }
    }

    /// Decodes a configuration.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for more than [`MAX_GUARDIANS`] or truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        let delay_blocks = r.read_u64()?;
        let limit = r.read_u64()?;
        let n = r.read_collection_len(32)?;
        if n > MAX_GUARDIANS {
            return Err(NodeError::Decode(format!("{n} vault guardians")));
        }
        let guardians = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        Ok(Self {
            delay_blocks,
            limit,
            guardians,
        })
    }
}

/// One vault action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VaultAction {
    /// Opt in, or queue a new policy.
    Configure(VaultConfig),
    /// Escrow `amount` for `to`, payable after the delay.
    Request {
        /// Recipient.
        to: [u8; 32],
        /// Amount.
        amount: u64,
    },
    /// Pay a matured request.
    Execute {
        /// The vault.
        owner: [u8; 32],
        /// The request.
        id: u64,
    },
    /// Cancel an open request, or [`RECONFIGURE_ID`] for a queued policy.
    Cancel {
        /// The vault.
        owner: [u8; 32],
        /// The request.
        id: u64,
    },
}

impl VaultAction {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        match self {
            Self::Configure(config) => {
                buf.push(CONFIGURE);
                config.encode_into(buf);
            }
            Self::Request { to, amount } => {
                buf.push(REQUEST);
                buf.extend_from_slice(to);
                buf.extend_from_slice(&amount.to_le_bytes());
            }
            Self::Execute { owner, id } | Self::Cancel { owner, id } => {
                buf.push(if matches!(self, Self::Execute { .. }) {
                    EXECUTE
                } else {
                    CANCEL
                });
                buf.extend_from_slice(owner);
                buf.extend_from_slice(&id.to_le_bytes());
            }
        }
    }

    /// Decodes one action.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for an unknown action or truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        match r.read_u8()? {
            CONFIGURE => Ok(Self::Configure(VaultConfig::decode(r)?)),
            REQUEST => Ok(Self::Request {
                to: r.read_array()?,
                amount: r.read_u64()?,
            }),
            EXECUTE => Ok(Self::Execute {
                owner: r.read_array()?,
                id: r.read_u64()?,
            }),
            CANCEL => Ok(Self::Cancel {
                owner: r.read_array()?,
                id: r.read_u64()?,
            }),
            t => Err(NodeError::Decode(format!("vault action {t}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn every_action_round_trips() {
        for a in [
            VaultAction::Configure(VaultConfig {
                delay_blocks: 10,
                limit: 500,
                guardians: vec![[1; 32], [2; 32]],
            }),
            VaultAction::Request {
                to: [3; 32],
                amount: 9_000,
            },
            VaultAction::Execute {
                owner: [4; 32],
                id: 0,
            },
            VaultAction::Cancel {
                owner: [4; 32],
                id: RECONFIGURE_ID,
            },
        ] {
            let mut buf = Vec::new();
            a.encode_into(&mut buf);
            let mut r = ByteReader::new(&buf);
            assert_eq!(VaultAction::decode(&mut r).unwrap(), a);
            r.finish().unwrap();
        }
        let mut too_many = Vec::new();
        VaultAction::Configure(VaultConfig {
            delay_blocks: 1,
            limit: 0,
            guardians: vec![[0; 32]; MAX_GUARDIANS + 1],
        })
        .encode_into(&mut too_many);
        assert!(VaultAction::decode(&mut ByteReader::new(&too_many)).is_err());
    }
}
