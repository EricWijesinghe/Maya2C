//! Capped-asset register, mint, burn and renounce (ADR-135 P-1, bit 41).
//!
//! Every rule is a refusal, never a no-op, and admission calls the same
//! activation check apply does ([`require_active`]), so a transaction the
//! mempool holds is one a block can carry (the property invariant 31 protects
//! for signatures).
//!
//! ## Money math, with its bounds
//!
//! Four comparisons and no multiplication, all on `U256`, nothing wider:
//!
//! - `cap <= MAX_ASSET_SUPPLY` (about 2^176) at registration, the ceiling
//!   `RegisterAsset` already enforces so a pool's scaled price cannot overflow.
//!   ADR-135 does not state it; without it a mint could take a pooled asset
//!   past the bound the DEX relies on.
//! - mint: `total_supply.checked_add(amount)`, then `<= cap`. The sum is at
//!   most `2 * MAX_ASSET_SUPPLY` when it exists; an `amount` near 2^256 makes
//!   `checked_add` fail rather than wrap under the cap (threat T-1).
//! - burn: the sender's balance `checked_sub(amount)` and
//!   `total_supply.checked_sub(amount)`. Supply is the sum of balances
//!   (the conservation guard checks it), so the second cannot fail when the
//!   first passed; it is checked anyway rather than assumed.
//! - the registration fee: one debit of a governed value below 2^252.
//!
//! Records are overlay writes under `d:asset:`, so `capture_undo` journals the
//! prior bytes and a reorg restores authority and cap with everything else
//! (threat T-3).

use maya_governance::params::ParameterKey;
use maya_stateless_core::amount::to_u256;
use primitive_types::U256;

use crate::core::TxKind;
use crate::core::asset_mint_payload::AssetMintAction;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::asset::{
    AssetId, AssetRecord, AssetRecordV2, NO_AUTHORITY, derive_asset_id, is_native, validate_symbol,
};
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::dex::asset_key;
use crate::state::shielded::FEE_SINK;

/// Refuses a capped-asset form before bit 41 activates; every other kind
/// passes. The one check both admission and apply call.
///
/// # Errors
///
/// [`NodeError::AssetMintInactive`] for tags 55-58 below the activation height.
pub fn require_active(kind: &TxKind, context: BlockContext) -> Result<()> {
    if matches!(kind, TxKind::AssetMint(_)) && !context.asset_mint_active() {
        return Err(NodeError::AssetMintInactive {
            height: context.height,
            activation: context.asset_mint_activation,
        });
    }
    Ok(())
}

const fn refused(reason: &'static str) -> NodeError {
    NodeError::AssetMintRefused { reason }
}

impl StateDB {
    /// Reads an asset's record from committed state as v2: a v1 record comes
    /// back capped at its supply with no authority.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure, or a decode error for a
    /// corrupt record.
    pub fn get_capped_asset(&self, asset: &AssetId) -> Result<Option<AssetRecordV2>> {
        self.raw_get(&asset_key(asset))?
            .map(|bytes| AssetRecordV2::decode(&bytes))
            .transpose()
    }

    /// Executes one capped-asset form for `sender`.
    pub(crate) fn apply_asset_mint(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        action: &AssetMintAction,
        context: BlockContext,
    ) -> Result<()> {
        require_active(&TxKind::AssetMint(*action), context)?;
        match action {
            AssetMintAction::RegisterCapped {
                symbol,
                total_supply,
                cap,
                authority,
            } => {
                self.charge_asset_registration_fee(overlay, sender, context)?;
                let record = AssetRecordV2 {
                    base: AssetRecord {
                        creator: *sender,
                        total_supply: *total_supply,
                        symbol: *symbol,
                    },
                    cap: *cap,
                    authority: *authority,
                };
                self.register_capped_asset(overlay, nonce, &record)
            }
            AssetMintAction::Mint { asset, to, amount } => {
                self.mint_asset(overlay, sender, asset, to, *amount)
            }
            AssetMintAction::Burn { asset, amount } => {
                self.burn_asset(overlay, sender, asset, *amount)
            }
            AssetMintAction::Renounce { asset } => self.renounce_mint(overlay, sender, asset),
        }
    }

    /// Burns the governed registration surcharge from `sender` once bit 41 is
    /// active. Zero by default (`AssetRegistrationFee`, pending Eric O-2), and
    /// nothing at all before activation, so `RegisterAsset`'s pre-v4 behaviour
    /// is untouched on every node.
    pub(crate) fn charge_asset_registration_fee(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        context: BlockContext,
    ) -> Result<()> {
        if !context.asset_mint_active() {
            return Ok(());
        }
        let fee = to_u256(self.parameter(overlay, ParameterKey::AssetRegistrationFee)?);
        if fee.is_zero() {
            return Ok(());
        }
        self.debit_native(overlay, sender, fee)?;
        self.credit_native(overlay, &FEE_SINK, fee)
    }

    fn register_capped_asset(
        &self,
        overlay: &mut Overlay,
        nonce: u64,
        record: &AssetRecordV2,
    ) -> Result<()> {
        let creator = record.base.creator;
        validate_symbol(&record.base.symbol)?;
        if record.authority != NO_AUTHORITY && record.authority != creator {
            return Err(refused("the authority must be the sender or none"));
        }
        if record.cap.is_zero() {
            return Err(refused("a cap of zero is an asset that can never exist"));
        }
        if record.base.total_supply > record.cap {
            return Err(refused("the initial supply is above the cap"));
        }
        if record.base.total_supply.is_zero() && record.authority == NO_AUTHORITY {
            return Err(refused(
                "no supply and no authority would be a name and nothing else",
            ));
        }
        if record.cap > to_u256(maya_dex::MAX_ASSET_SUPPLY) {
            return Err(refused(
                "cap above MAX_ASSET_SUPPLY (about 2^176) would overflow pool prices",
            ));
        }

        let asset = derive_asset_id(&creator, nonce, &record.base.symbol);
        if self.record(overlay, &asset_key(&asset))?.is_some() {
            return Err(NodeError::AssetExists(hex::encode(asset)));
        }
        Self::put_record(overlay, asset_key(&asset), record.encode().to_vec());
        self.credit_asset(overlay, &asset, &creator, record.base.total_supply)
    }

    /// The record of a registered, non-native asset, as v2.
    fn load_capped(&self, overlay: &Overlay, asset: &AssetId) -> Result<AssetRecordV2> {
        if is_native(asset) {
            return Err(refused("the native coin is not a capped asset"));
        }
        match self.record(overlay, &asset_key(asset))? {
            Some(bytes) => AssetRecordV2::decode(&bytes),
            None => Err(NodeError::UnknownAsset(hex::encode(asset))),
        }
    }

    fn require_authority(record: &AssetRecordV2, sender: &Address) -> Result<()> {
        if record.authority == NO_AUTHORITY {
            return Err(refused("the asset has no mint authority"));
        }
        if record.authority != *sender {
            return Err(refused("the sender is not the asset's mint authority"));
        }
        Ok(())
    }

    fn mint_asset(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        asset: &AssetId,
        to: &Address,
        amount: U256,
    ) -> Result<()> {
        if amount.is_zero() {
            return Err(refused("a mint of zero units"));
        }
        let mut record = self.load_capped(overlay, asset)?;
        Self::require_authority(&record, sender)?;
        let supply = record
            .base
            .total_supply
            .checked_add(amount)
            .ok_or(refused("the new supply overflows U256"))?;
        if supply > record.cap {
            return Err(refused("the new supply is above the cap"));
        }
        record.base.total_supply = supply;
        Self::put_record(overlay, asset_key(asset), record.encode().to_vec());
        self.credit_asset(overlay, asset, to, amount)
    }

    fn burn_asset(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        asset: &AssetId,
        amount: U256,
    ) -> Result<()> {
        // A zero burn would rewrite a v1 record as v2 for nothing (S-6's twin).
        if amount.is_zero() {
            return Err(refused("a burn of zero units"));
        }
        let mut record = self.load_capped(overlay, asset)?;
        self.debit_asset(overlay, asset, sender, amount)?;
        record.base.total_supply = record
            .base
            .total_supply
            .checked_sub(amount)
            .ok_or(refused("the burn is above the supply"))?;
        Self::put_record(overlay, asset_key(asset), record.encode().to_vec());
        Ok(())
    }

    fn renounce_mint(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        asset: &AssetId,
    ) -> Result<()> {
        let mut record = self.load_capped(overlay, asset)?;
        Self::require_authority(&record, sender)?;
        record.authority = NO_AUTHORITY;
        record.cap = record.base.total_supply;
        Self::put_record(overlay, asset_key(asset), record.encode().to_vec());
        Ok(())
    }
}
