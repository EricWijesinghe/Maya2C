//! P2P networking: gossipsub broadcast, Kademlia discovery, and the mempool
//! that gates inbound transactions.

pub mod behaviour;
pub mod evidence_tap;
pub mod gossip_score;
pub mod identity;
pub mod mempool;
pub mod node;
pub mod peer_health;
pub mod pq;
pub mod radio_gateway;
pub mod relay_key;
pub mod sim;
pub mod sync;
pub mod topics;

pub use behaviour::NodeBehaviour;
pub use evidence_tap::EvidenceTap;
pub use identity::{NODE_KEY_FILE, load_or_create as load_or_create_identity, peer_id_at};
pub use mempool::{Mempool, TxHash};
pub use node::{Node, NodeEvent, NodeHandle, RelayConfig, RelayIngress};
pub use peer_health::{GuardConfig, Offence, PeerReport};
pub use pq::{EpochClock, PqUpgrade, ROTATION_INTERVAL_BLOCKS, SessionStats};
pub use sim::{DelayStream, LATENCY_SWEEP, LatencyDial};
pub use topics::{BLOCKS_TOPIC, TXS_TOPIC, blocks_topic, txs_topic};
