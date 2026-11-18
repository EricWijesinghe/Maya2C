//! The v4 read methods (ADR-135 I8): one committed-state read each, so the
//! explorer and the gateway can show a name, an asset, an NFT or a schedule
//! without parsing state keys themselves. `null` means no such record.

use jsonrpsee::server::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;

use super::{RpcContext, decode_array, invalid_params, rejected};
use crate::rpc::v4_types::{AssetInfo, NameInfo, NftCollectionInfo, NftItemInfo, ScheduleInfo};

/// Registers `get_name`, `get_asset`, `get_nft_collection`, `get_nft_item`
/// and `get_schedule`.
pub(super) fn register(module: &mut RpcModule<RpcContext>) -> Result<(), ErrorObjectOwned> {
    module
        .register_method("get_name", |params, ctx, _| {
            // The label is checked by the same rule the decoder applies, so a
            // lookup can never reach a key no transaction could have written.
            let label: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let label = label.strip_suffix(".maya").unwrap_or(&label);
            crate::core::name_payload::validate_label(label.as_bytes())
                .map_err(|why| invalid_params(format!("name label: {why}")))?;
            let chain = ctx.chain();
            let record = chain
                .state()
                .get_name(label.as_bytes())
                .map_err(|e| rejected(e.to_string()))?;
            // Judged where the next transaction lands: an expired record stays
            // in state until someone registers the name again, and must not
            // read as a live target in the meantime.
            let next = chain.height().saturating_add(1);
            Ok::<_, ErrorObjectOwned>(record.as_ref().map(|r| NameInfo::at(r, next)))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_asset", |params, ctx, _| {
            let id_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let id = decode_array::<32>(&id_hex, "asset")?;
            let record = ctx
                .chain()
                .state()
                .get_capped_asset(&id)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(record.map(|r| AssetInfo::new(&id, &r)))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_nft_collection", |params, ctx, _| {
            let id_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let id = decode_array::<32>(&id_hex, "collection")?;
            let record = ctx
                .chain()
                .state()
                .get_nft_collection(&id)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(record.map(|r| NftCollectionInfo::new(&id, &r)))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_nft_item", |params, ctx, _| {
            let (id_hex, index): (String, u64) =
                params.parse().map_err(|e| invalid_params(e.to_string()))?;
            let id = decode_array::<32>(&id_hex, "collection")?;
            let item = ctx
                .chain()
                .state()
                .get_nft_item(&id, index)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(item.map(|i| NftItemInfo::new(&id, index, &i)))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_schedule", |params, ctx, _| {
            let id_hex: String = params.one().map_err(|e| invalid_params(e.to_string()))?;
            let id = decode_array::<32>(&id_hex, "schedule")?;
            let record = ctx
                .chain()
                .state()
                .get_schedule(&id)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(record.map(|r| ScheduleInfo::new(&id, &r)))
        })
        .map_err(|e| rejected(e.to_string()))?;
    Ok(())
}
