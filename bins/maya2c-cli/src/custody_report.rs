//! `maya2c custody-report` — a custodian's statement over a height range
//! (Master Prompt 28 §3, "compliance-ready reporting for custodians").
//!
//! For each account: the balance before the first block, every block that
//! moved it, the balance after the last, and the vault policy at the tip.
//! The statement **reconciles or fails**: each movement's `before` must equal
//! the running balance and the last `after` must equal the closing balance,
//! so a missing or pruned block is an error, never a silent gap.
//!
//! It is anchored to the canonical block id at `to`, read before anything else
//! and again after everything else: a reorganisation touching any block at or
//! below `to` replaces that block, so an unchanged anchor means every read came
//! from one history. It carries a SHA-256 digest
//! of its canonical JSON, so an auditor re-running it against any honest node
//! at the same range gets the same bytes. It is a read of the chain, not an
//! attestation: it proves nothing the auditor's own node would not show.

use custom_l1_node::rpc::types::BalanceChangesInfo;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// One block that moved an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Movement {
    /// Block height.
    pub height: u64,
    /// Block id, hex.
    pub block_id: String,
    /// Balance before the block.
    pub before: u64,
    /// Balance after it.
    pub after: u64,
}

/// One account's statement.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Statement {
    /// Address, hex.
    pub address: String,
    /// Balance before block `from`.
    pub opening: u64,
    /// Balance after block `to`.
    pub closing: u64,
    /// Every block in the range that moved it, ascending.
    pub movements: Vec<Movement>,
    /// Vault policy at the tip (`vault_get`), or null.
    pub vault: serde_json::Value,
}

/// A report over `[from, to]`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    /// First block covered.
    pub from: u64,
    /// Block id at `from`.
    pub from_block: String,
    /// Last block covered.
    pub to: u64,
    /// Block id at `to`.
    pub to_block: String,
    /// One statement per requested account, in request order.
    pub accounts: Vec<Statement>,
}

impl Report {
    /// Canonical JSON: the digest is over exactly these bytes.
    ///
    /// # Errors
    ///
    /// Serialization failure (not expected for these types).
    pub fn canonical(&self) -> anyhow::Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// SHA-256 of [`Report::canonical`], hex.
    ///
    /// # Errors
    ///
    /// As [`Report::canonical`].
    pub fn digest(&self) -> anyhow::Result<String> {
        Ok(hex::encode(Sha256::digest(self.canonical()?)))
    }

    /// One row per movement: `address,height,block_id,before,after,delta`.
    #[must_use]
    pub fn csv(&self) -> String {
        let rows = self.accounts.iter().flat_map(|s| {
            s.movements.iter().map(move |m| {
                let delta = i128::from(m.after) - i128::from(m.before);
                format!(
                    "{},{},{},{},{},{delta}\n",
                    s.address, m.height, m.block_id, m.before, m.after
                )
            })
        });
        std::iter::once("address,height,block_id,before,after,delta\n".to_owned())
            .chain(rows)
            .collect()
    }
}

/// Builds and reconciles one statement from the blocks' balance changes.
///
/// # Errors
///
/// A movement whose `before` is not the running balance, or a closing
/// balance the movements do not reach: the data has a gap.
pub fn reconcile(
    address: &str,
    opening: u64,
    closing: u64,
    blocks: &[BalanceChangesInfo],
) -> anyhow::Result<Vec<Movement>> {
    let mut running = opening;
    let mut movements = Vec::new();
    for block in blocks {
        for c in block.changes.iter().filter(|c| c.address == address) {
            anyhow::ensure!(
                c.before == running,
                "{address} at height {}: block says {} before, statement has {running}",
                block.height,
                c.before
            );
            running = c.after;
            movements.push(Movement {
                height: block.height,
                block_id: block.block_id.clone(),
                before: c.before,
                after: c.after,
            });
        }
    }
    anyhow::ensure!(
        running == closing,
        "{address}: movements end at {running}, the chain says {closing}"
    );
    Ok(movements)
}

async fn balance_at(
    client: &HttpClient,
    address: &str,
    height: u64,
) -> anyhow::Result<(u64, String)> {
    let v: serde_json::Value = client
        .request("get_balance_at_height", rpc_params![address, height])
        .await?;
    let balance = v["balance"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("get_balance_at_height: no balance in {v}"))?;
    let id = v["block_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("get_balance_at_height: no block_id in {v}"))?;
    Ok((balance, id.to_owned()))
}

/// Largest range one report covers; the node's own rewind bound is usually
/// lower.
pub const MAX_RANGE: u64 = 100_000;
const BLOCK_ID_HEX: usize = 64;

/// Refuses anything but `len` lowercase hex characters, so no RPC answer or
/// argument can put a delimiter, quote or formula into the CSV.
fn hex_field(what: &str, text: &str, len: usize) -> anyhow::Result<String> {
    anyhow::ensure!(
        text.len() == len
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{what} is not {len} lowercase hex characters: {text:?}"
    );
    Ok(text.to_owned())
}

async fn changes_at(client: &HttpClient, height: u64) -> anyhow::Result<BalanceChangesInfo> {
    let b: BalanceChangesInfo = client
        .request("get_balance_changes", rpc_params![height])
        .await?;
    hex_field("block id", &b.block_id, BLOCK_ID_HEX)?;
    for c in &b.changes {
        hex_field("address", &c.address, BLOCK_ID_HEX)?;
    }
    Ok(b)
}

async fn statement(
    client: &HttpClient,
    address: &str,
    (from, to): (u64, u64),
    blocks: &[BalanceChangesInfo],
) -> anyhow::Result<Statement> {
    let (opening, _) = balance_at(client, address, from - 1).await?;
    let (closing, _) = balance_at(client, address, to).await?;
    let movements = reconcile(address, opening, closing, blocks)?;
    let vault: serde_json::Value = client.request("vault_get", rpc_params![address]).await?;
    Ok(Statement {
        address: address.to_owned(),
        opening,
        closing,
        movements,
        vault,
    })
}

/// Fetches and reconciles a report from a node. `from` must be at least 1:
/// the opening balance is read at `from - 1`.
///
/// # Errors
///
/// An unreachable node, a range outside what the node keeps (pruned or
/// beyond its rewind bound) or over [`MAX_RANGE`], an address or block id
/// that is not 32 bytes of hex, a reorganisation during the reads, or a
/// statement that does not reconcile.
pub async fn fetch(rpc: &str, addresses: &[String], from: u64, to: u64) -> anyhow::Result<Report> {
    anyhow::ensure!(
        from >= 1 && from <= to,
        "need 1 <= from <= to, got {from}..={to}"
    );
    anyhow::ensure!(
        to - from < MAX_RANGE,
        "more than {MAX_RANGE} blocks in one report"
    );
    for address in addresses {
        hex_field("address", address, BLOCK_ID_HEX)?;
    }
    let client = HttpClientBuilder::default().build(rpc)?;
    let anchor = changes_at(&client, to).await?.block_id;
    let mut blocks = Vec::new();
    for height in from..=to {
        blocks.push(changes_at(&client, height).await?);
    }
    let mut accounts = Vec::new();
    for address in addresses {
        accounts.push(statement(&client, address, (from, to), &blocks).await?);
    }
    let after = changes_at(&client, to).await?.block_id;
    anyhow::ensure!(
        after == anchor && blocks.last().is_some_and(|b| b.block_id == anchor),
        "the chain reorganised at or below height {to} during the report"
    );
    let from_block = blocks
        .first()
        .map(|b| b.block_id.clone())
        .unwrap_or_default();
    Ok(Report {
        from,
        from_block,
        to,
        to_block: anchor,
        accounts,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use custom_l1_node::rpc::types::BalanceChangeInfo;

    fn block(height: u64, changes: &[(&str, u64, u64)]) -> BalanceChangesInfo {
        BalanceChangesInfo {
            height,
            block_id: format!("{height:064x}"),
            changes: changes
                .iter()
                .map(|&(a, before, after)| BalanceChangeInfo {
                    address: a.into(),
                    before,
                    after,
                })
                .collect(),
        }
    }

    #[test]
    fn movements_reconcile_and_a_gap_is_an_error() {
        let blocks = [
            block(5, &[("aa", 100, 70), ("bb", 0, 30)]),
            block(6, &[("bb", 30, 20)]),
            block(7, &[("aa", 70, 80)]),
        ];
        let m = reconcile("aa", 100, 80, &blocks).unwrap();
        assert_eq!(m.iter().map(|m| m.height).collect::<Vec<_>>(), [5, 7]);
        assert!(
            reconcile("aa", 100, 81, &blocks).is_err(),
            "closing disagrees"
        );
        assert!(
            reconcile("aa", 100, 80, &blocks[1..]).is_err(),
            "block 5 missing"
        );
        assert!(
            reconcile("cc", 9, 9, &blocks).unwrap().is_empty(),
            "untouched account"
        );
    }

    #[test]
    fn the_digest_is_over_the_canonical_bytes_and_csv_carries_signed_deltas() {
        let blocks = [block(5, &[("aa", 100, 70)])];
        let statement = Statement {
            address: "aa".into(),
            opening: 100,
            closing: 70,
            movements: reconcile("aa", 100, 70, &blocks).unwrap(),
            vault: serde_json::Value::Null,
        };
        let report = Report {
            from: 5,
            from_block: "x".into(),
            to: 5,
            to_block: "x".into(),
            accounts: vec![statement],
        };
        assert_eq!(report.digest().unwrap(), report.clone().digest().unwrap());
        assert_eq!(
            report.digest().unwrap(),
            hex::encode(Sha256::digest(report.canonical().unwrap()))
        );
        assert!(
            report
                .csv()
                .ends_with(&format!("aa,5,{},100,70,-30\n", "0".repeat(63) + "5"))
        );
    }

    #[test]
    fn only_hex_reaches_the_csv() {
        let good = "ab".repeat(32);
        assert_eq!(hex_field("id", &good, BLOCK_ID_HEX).unwrap(), good);
        for bad in [
            format!("={}", &good[1..]),
            format!("{},x", &good[..62]),
            good.to_uppercase(),
            good[..62].to_owned(),
        ] {
            assert!(hex_field("id", &bad, BLOCK_ID_HEX).is_err(), "{bad}");
        }
    }
}
