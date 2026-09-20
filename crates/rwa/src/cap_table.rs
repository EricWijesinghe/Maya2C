//! Who holds a token, and how much.
//!
//! ## Paged, for the reason the revocation bitmap is
//!
//! A cap table with ten thousand holders is a record that grows without a limit
//! anybody wrote down, and every write to it journals the whole thing. So it is
//! pages of [`HOLDERS_PER_PAGE`], each a fixed shape, and an issuer with more
//! holders has more pages.
//!
//! That also bounds what one distribution touches: the fan-out is over pages
//! the caller names, so "ten thousand holders" is a number in a test rather
//! than an unbounded loop inside consensus.
//!
//! ## The page carries weights, not percentages
//!
//! A holder's share is `units / Σ units`, computed at distribution time.
//! Storing a percentage would mean storing a rounded number and then rounding
//! again at payout — two roundings where one is already the hard part, and the
//! stored one would drift every time somebody's holding changed.

use crate::error::{Error, Result};
use crate::{ADDRESS_BYTES, Address, DIGEST_BYTES, Digest};

/// Holders one page carries.
///
/// 256, which is 12 KB a page. Small enough that a page rewrite is a reasonable
/// journal entry, large enough that ten thousand holders is forty pages rather
/// than a thousand.
pub const HOLDERS_PER_PAGE: usize = 256;

/// Bytes of one holder entry: the address and the units held.
const HOLDER_BYTES: usize = ADDRESS_BYTES + 8;

/// Bytes of a page's framing: the asset, the page number and the count.
const PAGE_OVERHEAD: usize = DIGEST_BYTES + 4 + 2;

/// One line of a cap table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holder {
    /// Who holds.
    pub address: Address,
    /// How many units. The weight a distribution is in proportion to.
    pub units: u64,
}

/// One page of a token's cap table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapTablePage {
    /// Which token.
    pub asset: Digest,
    /// Which page.
    pub page: u32,
    /// The holders, ascending by address.
    holders: Vec<Holder>,
}

impl CapTablePage {
    /// An empty page.
    #[must_use]
    pub fn empty(asset: Digest, page: u32) -> Self {
        Self {
            asset,
            page,
            holders: Vec::new(),
        }
    }

    /// A page from holders.
    ///
    /// Sorted by address and checked for duplicates. Sorting here rather than
    /// trusting the caller is what makes the encoding canonical: two pages with
    /// the same holders in different orders would be two different records
    /// under one state root, and the root commits to bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] past [`HOLDERS_PER_PAGE`],
    /// [`Error::Inconsistent`] for a repeated address — one holder with two
    /// lines would be paid twice — and [`Error::Invalid`] for a zero holding,
    /// which is an absent holder written down.
    pub fn new(asset: Digest, page: u32, mut holders: Vec<Holder>) -> Result<Self> {
        if holders.len() > HOLDERS_PER_PAGE {
            return Err(Error::Oversized {
                what: "cap table holders",
                found: holders.len(),
                limit: HOLDERS_PER_PAGE,
            });
        }
        if holders.iter().any(|holder| holder.units == 0) {
            return Err(Error::Invalid {
                field: "units",
                reason: "a holder of zero units is an absent holder written down".to_owned(),
            });
        }
        holders.sort_unstable_by_key(|holder| holder.address);
        if holders
            .windows(2)
            .any(|pair| pair[0].address == pair[1].address)
        {
            return Err(Error::Inconsistent {
                what: "cap table",
                reason: "one address holds two lines and would be paid twice".to_owned(),
            });
        }
        Ok(Self {
            asset,
            page,
            holders,
        })
    }

    /// The holders, ascending by address.
    #[must_use]
    pub fn holders(&self) -> &[Holder] {
        &self.holders
    }

    /// Total units on this page.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Inconsistent`] if the units overflow a `u64` — a page
    /// that cannot state its own total is one no distribution can be in
    /// proportion to.
    pub fn total_units(&self) -> Result<u64> {
        self.holders
            .iter()
            .try_fold(0u64, |sum, holder| sum.checked_add(holder.units))
            .ok_or(Error::Inconsistent {
                what: "cap table",
                reason: "holdings overflow a u64".to_owned(),
            })
    }

    /// What one address holds, or zero.
    #[must_use]
    pub fn units_of(&self, address: &Address) -> u64 {
        self.holders
            .binary_search_by_key(address, |holder| holder.address)
            .map_or(0, |index| self.holders[index].units)
    }

    /// Moves units between two addresses on this page.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Inconsistent`] if the sender does not hold enough, and
    /// [`Error::Oversized`] if crediting a new holder would exceed the page.
    pub fn transfer(&mut self, from: &Address, to: &Address, units: u64) -> Result<()> {
        if units == 0 {
            return Err(Error::Invalid {
                field: "units",
                reason: "a transfer of nothing is not a transfer".to_owned(),
            });
        }
        let sender = self
            .holders
            .binary_search_by_key(from, |holder| holder.address)
            .map_err(|_| Error::Inconsistent {
                what: "transfer",
                reason: "the sender holds none of this asset".to_owned(),
            })?;
        let remaining =
            self.holders[sender]
                .units
                .checked_sub(units)
                .ok_or(Error::Inconsistent {
                    what: "transfer",
                    reason: "the sender holds less than the transfer".to_owned(),
                })?;

        match self
            .holders
            .binary_search_by_key(to, |holder| holder.address)
        {
            Ok(recipient) => {
                self.holders[recipient].units = self.holders[recipient]
                    .units
                    .checked_add(units)
                    .ok_or(Error::Inconsistent {
                        what: "transfer",
                        reason: "the recipient's holding would overflow".to_owned(),
                    })?;
            }
            Err(insertion) => {
                if self.holders.len() >= HOLDERS_PER_PAGE {
                    return Err(Error::Oversized {
                        what: "cap table holders",
                        found: self.holders.len() + 1,
                        limit: HOLDERS_PER_PAGE,
                    });
                }
                self.holders.insert(
                    insertion,
                    Holder {
                        address: *to,
                        units,
                    },
                );
            }
        }

        // Recompute the sender's position: inserting the recipient may have
        // shifted it. A stale index here would credit the wrong holder, which
        // is the kind of bug that balances and is still wrong.
        let sender = self
            .holders
            .binary_search_by_key(from, |holder| holder.address)
            .expect("the sender was found above and was not removed");
        if remaining == 0 {
            self.holders.remove(sender);
        } else {
            self.holders[sender].units = remaining;
        }
        Ok(())
    }

    /// The canonical encoding.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PAGE_OVERHEAD + self.holders.len() * HOLDER_BYTES);
        out.extend_from_slice(&self.asset);
        out.extend_from_slice(&self.page.to_le_bytes());
        out.extend_from_slice(&(self.holders.len() as u16).to_le_bytes());
        for holder in &self.holders {
            out.extend_from_slice(&holder.address);
            out.extend_from_slice(&holder.units.to_le_bytes());
        }
        out
    }

    /// Reads a page.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a truncated record, trailing bytes, or
    /// a count that disagrees with what arrived, and whatever
    /// [`CapTablePage::new`] returns for holders outside their bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < PAGE_OVERHEAD {
            return Err(Error::Malformed {
                what: "cap table page",
                reason: format!("{} bytes is shorter than the framing", bytes.len()),
            });
        }
        let asset: Digest = bytes[..DIGEST_BYTES].try_into().expect("sized");
        let page = u32::from_le_bytes(
            bytes[DIGEST_BYTES..DIGEST_BYTES + 4]
                .try_into()
                .expect("sized"),
        );
        let count = u16::from_le_bytes(
            bytes[DIGEST_BYTES + 4..PAGE_OVERHEAD]
                .try_into()
                .expect("sized"),
        ) as usize;

        let body = &bytes[PAGE_OVERHEAD..];
        if body.len() != count * HOLDER_BYTES {
            return Err(Error::Malformed {
                what: "cap table page",
                reason: format!("declares {count} holders and carries {} bytes", body.len()),
            });
        }
        let (whole, _) = body.as_chunks::<HOLDER_BYTES>();
        let holders = whole
            .iter()
            .map(|chunk| Holder {
                address: chunk[..ADDRESS_BYTES].try_into().expect("sized"),
                units: u64::from_le_bytes(chunk[ADDRESS_BYTES..].try_into().expect("sized")),
            })
            .collect();
        Self::new(asset, page, holders)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn holder(fill: u8, units: u64) -> Holder {
        Holder {
            address: [fill; ADDRESS_BYTES],
            units,
        }
    }

    fn page() -> CapTablePage {
        CapTablePage::new([9; DIGEST_BYTES], 0, vec![holder(3, 30), holder(1, 10)]).expect("page")
    }

    #[test]
    fn a_page_survives_its_wire_format() {
        let original = page();
        assert_eq!(
            CapTablePage::decode(&original.encode()).expect("decode"),
            original
        );
    }

    #[test]
    fn holders_are_stored_in_address_order_whatever_order_they_arrive_in() {
        // Two pages with the same holders in different orders would be two
        // records under one state root, and the root commits to bytes.
        let ascending = CapTablePage::new([9; DIGEST_BYTES], 0, vec![holder(1, 10), holder(3, 30)])
            .expect("page");
        assert_eq!(page().encode(), ascending.encode());
        assert_eq!(page().holders()[0].address, [1; ADDRESS_BYTES]);
    }

    #[test]
    fn one_address_cannot_hold_two_lines() {
        // It would be paid twice by every distribution.
        assert!(
            CapTablePage::new([9; DIGEST_BYTES], 0, vec![holder(1, 10), holder(1, 5)]).is_err()
        );
    }

    #[test]
    fn a_zero_holding_is_refused() {
        assert!(CapTablePage::new([9; DIGEST_BYTES], 0, vec![holder(1, 0)]).is_err());
    }

    #[test]
    fn a_transfer_moves_units_and_keeps_the_total() {
        let mut page = page();
        let before = page.total_units().expect("total");
        page.transfer(&[3; ADDRESS_BYTES], &[7; ADDRESS_BYTES], 12)
            .expect("transfer");
        assert_eq!(page.total_units().expect("total"), before);
        assert_eq!(page.units_of(&[3; ADDRESS_BYTES]), 18);
        assert_eq!(page.units_of(&[7; ADDRESS_BYTES]), 12);
    }

    #[test]
    fn a_holder_paid_down_to_nothing_leaves_the_table() {
        // Otherwise a distribution iterates rows that hold nothing, and the
        // page fills with holders who left.
        let mut page = page();
        page.transfer(&[1; ADDRESS_BYTES], &[3; ADDRESS_BYTES], 10)
            .expect("transfer");
        assert_eq!(page.units_of(&[1; ADDRESS_BYTES]), 0);
        assert_eq!(page.holders().len(), 1);
        assert_eq!(page.units_of(&[3; ADDRESS_BYTES]), 40);
    }

    #[test]
    fn inserting_a_recipient_does_not_shift_the_wrong_sender() {
        // The bug this exists for: the recipient sorts before the sender, the
        // sender's index moves, and a stale index debits somebody else. It
        // balances and it is still wrong.
        let mut page = CapTablePage::new([9; DIGEST_BYTES], 0, vec![holder(5, 50), holder(9, 90)])
            .expect("page");
        page.transfer(&[9; ADDRESS_BYTES], &[1; ADDRESS_BYTES], 40)
            .expect("transfer");
        assert_eq!(page.units_of(&[1; ADDRESS_BYTES]), 40);
        assert_eq!(page.units_of(&[9; ADDRESS_BYTES]), 50);
        assert_eq!(
            page.units_of(&[5; ADDRESS_BYTES]),
            50,
            "an untouched holder moved"
        );
    }

    #[test]
    fn a_transfer_the_sender_cannot_cover_is_refused_and_changes_nothing() {
        let mut page = page();
        let before = page.clone();
        assert!(
            page.transfer(&[1; ADDRESS_BYTES], &[7; ADDRESS_BYTES], 999)
                .is_err()
        );
        assert_eq!(page, before);
        assert!(
            page.transfer(&[8; ADDRESS_BYTES], &[7; ADDRESS_BYTES], 1)
                .is_err()
        );
        assert_eq!(page, before);
    }

    #[test]
    fn a_full_page_refuses_a_new_holder() {
        let holders: Vec<Holder> = (0..HOLDERS_PER_PAGE)
            .map(|index| {
                let mut address = [0u8; ADDRESS_BYTES];
                address[..2].copy_from_slice(&(index as u16).to_le_bytes());
                Holder { address, units: 10 }
            })
            .collect();
        let mut page = CapTablePage::new([9; DIGEST_BYTES], 0, holders).expect("page");
        assert!(
            page.transfer(&[0; ADDRESS_BYTES], &[0xff; ADDRESS_BYTES], 1)
                .is_err()
        );
        // But a transfer to somebody already on it is fine.
        let mut second = [0u8; ADDRESS_BYTES];
        second[..2].copy_from_slice(&1u16.to_le_bytes());
        assert!(page.transfer(&[0; ADDRESS_BYTES], &second, 1).is_ok());
    }

    #[test]
    fn a_truncated_or_padded_page_is_refused() {
        let encoded = page().encode();
        for len in 0..encoded.len() {
            assert!(CapTablePage::decode(&encoded[..len]).is_err(), "{len}");
        }
        let mut padded = encoded.clone();
        padded.push(0);
        assert!(CapTablePage::decode(&padded).is_err());
    }

    #[test]
    fn a_page_past_its_holder_limit_is_refused() {
        let holders: Vec<Holder> = (0..=HOLDERS_PER_PAGE)
            .map(|index| {
                let mut address = [0u8; ADDRESS_BYTES];
                address[..2].copy_from_slice(&(index as u16).to_le_bytes());
                Holder { address, units: 1 }
            })
            .collect();
        assert!(CapTablePage::new([9; DIGEST_BYTES], 0, holders).is_err());
    }
}
