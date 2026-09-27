//! The unified state mapping: one Maya account, one identity in every engine.
//!
//! Maya addresses are 32 bytes; EVM addresses are 20. The EVM address of a
//! Maya account is the last 20 bytes of a domain-separated BLAKE3 of it —
//! a hash, not a truncation, so no two Maya accounts that differ only in
//! their first 12 bytes can collide on purpose. Balances are the same number
//! in every engine: one Maya base unit is one wei and one lamport. What
//! engines add on top (EVM storage, SBF account data) stays per engine.

/// A Maya account address.
pub type MayaAddress = [u8; 32];

/// The EVM address of a Maya account.
#[must_use]
pub fn evm_address(maya: &MayaAddress) -> [u8; 20] {
    let h = blake3::derive_key("maya2c multivm evm address v1", maya);
    let mut out = [0u8; 20];
    out.copy_from_slice(&h[12..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_maya_accounts_map_to_distinct_evm_addresses() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a[0] = 1; // differ only in the first 12 bytes: a truncation would collide
        b[0] = 2;
        assert_ne!(evm_address(&a), evm_address(&b));
        assert_eq!(evm_address(&a), evm_address(&a), "deterministic");
    }
}
