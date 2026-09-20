//! [`ThreatSource`] over a co-located node's JSON-RPC.
//!
//! Point it at a node on this host. `threat_peer_addresses` answers only when
//! the node was started with its handle wired into RPC, and should be bound to
//! a local interface: it lists where peers connect from.

use std::net::IpAddr;

use async_trait::async_trait;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

use custom_l1_node::rpc::{PeerAddressInfo, ThreatIndicatorInfo};
use maya_threat_intel::{Author, INDICATOR_BYTES, ThreatIndicator};

use crate::error::{FirewallError, Result};
use crate::source::{Observation, ThreatSource};

/// A node reached over HTTP JSON-RPC.
#[derive(Debug, Clone)]
pub struct RpcSource {
    client: HttpClient,
    url: String,
}

impl RpcSource {
    /// Connects to a node's JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// [`FirewallError::Rpc`] if the URL is unusable.
    pub fn connect(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| FirewallError::Rpc(format!("connecting to {url}: {e}")))?;
        Ok(Self {
            client,
            url: url.to_owned(),
        })
    }

    async fn call<T: serde::de::DeserializeOwned>(&self, method: &str) -> Result<T> {
        self.client
            .request(method, rpc_params![])
            .await
            .map_err(|e| FirewallError::Rpc(format!("{} {method}: {e}", self.url)))
    }
}

#[async_trait]
impl ThreatSource for RpcSource {
    async fn observe(&self) -> Result<Observation> {
        let height: u64 = self.call("get_tip_height").await?;
        let indicators: Vec<ThreatIndicatorInfo> = self.call("threat_indicators").await?;
        let peers: Vec<PeerAddressInfo> = self.call("threat_peer_addresses").await?;
        Ok(Observation {
            height,
            indicators: indicators
                .iter()
                .map(indicator_from_info)
                .collect::<Result<_>>()?,
            addresses: peers
                .iter()
                .filter_map(|info| address_from_info(info).transpose())
                .collect::<Result<_>>()?,
        })
    }
}

/// The stored indicator an RPC response describes, refused unless it is one
/// the chain could hold.
///
/// # Errors
///
/// [`FirewallError::Rpc`] for a malformed author or a non-canonical record.
pub fn indicator_from_info(info: &ThreatIndicatorInfo) -> Result<(Author, ThreatIndicator)> {
    let author = author_from_hex(&info.author)?;
    let claimed = ThreatIndicator {
        score: info.score,
        first_height: info.first_height,
        last_height: info.last_height,
        offences: info.offences,
    };
    let encoded: [u8; INDICATOR_BYTES] = claimed.encode();
    let indicator = ThreatIndicator::decode(&encoded)
        .map_err(|e| FirewallError::Rpc(format!("indicator for {}: {e}", info.author)))?;
    Ok((author, indicator))
}

/// The author and address a peer entry names, or `None` for a peer whose
/// identity is not ed25519 and so can never carry an indicator.
///
/// # Errors
///
/// [`FirewallError::Rpc`] for a malformed author or address.
pub fn address_from_info(info: &PeerAddressInfo) -> Result<Option<(Author, IpAddr)>> {
    let Some(author) = info.author.as_deref() else {
        return Ok(None);
    };
    let ip = info.ip.parse().map_err(|e| {
        FirewallError::Rpc(format!("peer {} address {:?}: {e}", info.peer_id, info.ip))
    })?;
    Ok(Some((author_from_hex(author)?, ip)))
}

fn author_from_hex(text: &str) -> Result<Author> {
    let bytes =
        hex::decode(text).map_err(|e| FirewallError::Rpc(format!("author {text:?}: {e}")))?;
    Author::try_from(bytes.as_slice())
        .map_err(|_| FirewallError::Rpc(format!("author {text:?} is not 32 bytes")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use maya_threat_intel::OffenceKind;

    use super::*;

    #[test]
    fn an_indicator_survives_the_rpc_round_trip() {
        let author = [0xAA; 32];
        let indicator = ThreatIndicator::observe(None, OffenceKind::InvalidSignature, 12);
        let info = ThreatIndicatorInfo::new(&author, &indicator, 12);
        assert_eq!(
            indicator_from_info(&info).expect("canonical"),
            (author, indicator)
        );

        let forged = ThreatIndicatorInfo {
            offences: 0,
            ..info
        };
        assert!(indicator_from_info(&forged).is_err());
    }

    #[test]
    fn a_peer_without_an_ed25519_author_is_skipped_and_a_bad_address_refused() {
        let skipped = PeerAddressInfo {
            peer_id: "p".to_owned(),
            author: None,
            ip: "not an ip".to_owned(),
        };
        assert_eq!(address_from_info(&skipped).expect("skipped"), None);

        let parsed = PeerAddressInfo {
            author: Some(hex::encode([3u8; 32])),
            ip: "2001:db8::1".to_owned(),
            ..skipped.clone()
        };
        let (author, ip) = address_from_info(&parsed).expect("valid").expect("some");
        assert_eq!(author, [3; 32]);
        assert!(ip.is_ipv6());

        let bad = PeerAddressInfo {
            ip: "999.1.1.1".to_owned(),
            ..parsed
        };
        assert!(address_from_info(&bad).is_err());
    }
}
