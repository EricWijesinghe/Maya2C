//! Sends a transfer the way the desktop wallet does, against a live node or
//! gateway: read the account and the fee terms online, sign offline with the
//! collector as the fee output, broadcast. DEVNET ONLY: the key comes from the
//! well-known test phrase below, which anyone can spend from.
//!
//! ```text
//! cargo run -p maya-wallet-core --example devnet_send -- address
//! cargo run -p maya-wallet-core --example devnet_send -- <url> <recipient> <amount>
//! ```

use maya_wallet_core::hd::{self, DerivationPath, seed_from_mnemonic};
use maya_wallet_core::payment::{priced_fee_tiers, transfer_size};
use maya_wallet_core::wallet::{SignTerms, Wallet, broadcast, chain_info, fee_quote, fetch_account};

const DEVNET_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                             abandon abandon abandon about";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seed = seed_from_mnemonic(DEVNET_PHRASE, "")?;
    let key = hd::signing_key_at(seed.as_ref(), &DerivationPath::account(0, 0))?;
    let sender = hex::encode(key.address());

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("address") {
        println!("{sender}");
        return Ok(());
    }
    let [url, recipient, amount] = args.as_slice() else {
        return Err("usage: devnet_send address | <url> <recipient> <amount>".into());
    };
    let amount: u64 = amount.parse()?;

    let (balance, nonce) = fetch_account(url, &sender).await?;
    let quote = fee_quote(url).await?;
    let chain = chain_info(url).await?;
    println!("sender:   {sender}\nbalance:  {balance}\nnonce:    {nonce}");
    println!(
        "fees:     active={} base_fee={} collector={}",
        quote.active, quote.base_fee, quote.collector
    );

    // Signs only through the confirm-hash gate, as the GUI does: the hash is the
    // confirmation of these exact terms, so nothing differs between screen and bytes.
    let fee = if quote.active {
        let size = transfer_size(&key, &quote.collector, nonce, &chain)?;
        let [_, standard, _] = priced_fee_tiers(quote.base_fee, size)?;
        println!("size:     {size} bytes -> Standard fee {standard}");
        standard
    } else {
        primitive_types::U256::zero()
    };
    let terms = SignTerms {
        fee,
        fee_to: quote.active.then_some(quote.collector.as_str()),
        nonce,
        chain: &chain,
    };
    let amount = primitive_types::U256::from(amount);
    let mut wallet = Wallet::from_seed(seed);
    let confirm = wallet.confirm(0, recipient, amount, &terms)?;
    println!("confirm:  {}", confirm.description);
    let transfer = wallet.sign(0, recipient, amount, &terms, &confirm.signing_hash)?;
    let txid = broadcast(url, &transfer.raw_hex).await?;
    println!("txid:     {txid}");
    Ok(())
}
