//! Gossipsub's own peer scoring, configured to punish what is provable and
//! nothing else.
//!
//! [`crate::network::peer_health`] decides quarantine. This is the second,
//! cheaper layer: gossipsub keeps a score per peer for mesh management, and a
//! peer below its thresholds stops receiving gossip from us, then stops being
//! heard (graylisting). Left at its defaults it would also score *quietness* —
//! mesh delivery deficits, time in mesh, slow delivery — and a quiet or distant
//! honest peer would sink. So those components are switched off and only P4,
//! invalid message deliveries, carries weight: a peer loses score here only for
//! messages this node rejected.

use std::collections::HashMap;

use libp2p::gossipsub::{PeerScoreParams, PeerScoreThresholds, TopicScoreParams};

use crate::network::topics::{blocks_topic, txs_topic};

/// P4 weight. Gossipsub squares the invalid-delivery counter, so one rejected
/// message costs 50 and puts the peer below the gossip and publish thresholds;
/// two cost 200 and graylist it.
const INVALID_DELIVERY_WEIGHT: f64 = -50.0;

/// Fraction of the invalid-delivery counter kept per decay interval (1 s).
const INVALID_DELIVERY_DECAY: f64 = 0.9;

/// Per-topic parameters: P4 only.
fn topic() -> TopicScoreParams {
    TopicScoreParams {
        topic_weight: 1.0,
        time_in_mesh_weight: 0.0,
        first_message_deliveries_weight: 0.0,
        mesh_message_deliveries_weight: 0.0,
        mesh_failure_penalty_weight: 0.0,
        invalid_message_deliveries_weight: INVALID_DELIVERY_WEIGHT,
        invalid_message_deliveries_decay: INVALID_DELIVERY_DECAY,
        ..TopicScoreParams::default()
    }
}

/// Score parameters for both topics.
#[must_use]
pub fn params() -> PeerScoreParams {
    let topics: HashMap<_, _> = [txs_topic().hash(), blocks_topic().hash()]
        .into_iter()
        .map(|hash| (hash, topic()))
        .collect();
    PeerScoreParams {
        topics,
        // No application score: quarantine is enforced by blacklisting, not
        // by feeding a number back in here.
        app_specific_weight: 0.0,
        // Every memory-transport peer shares one "address", and a NAT puts
        // many honest peers behind one on a real network.
        ip_colocation_factor_weight: 0.0,
        // A slow peer is not a Byzantine one (see `peer_health`).
        slow_peer_weight: 0.0,
        ..PeerScoreParams::default()
    }
}

/// Gossipsub's default thresholds, which the P4 weight above is sized against.
#[must_use]
pub fn thresholds() -> PeerScoreThresholds {
    PeerScoreThresholds::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parameters_are_ones_gossipsub_accepts() {
        assert_eq!(params().validate(), Ok(()));
        assert_eq!(thresholds().validate(), Ok(()));
    }

    #[test]
    fn two_invalid_messages_reach_the_graylist_and_quiet_components_are_off() {
        let topic = topic();
        let two = topic.topic_weight * INVALID_DELIVERY_WEIGHT * 2.0 * 2.0;
        assert!(two <= thresholds().graylist_threshold);
        assert_eq!(topic.mesh_message_deliveries_weight, 0.0);
        assert_eq!(topic.mesh_failure_penalty_weight, 0.0);
        assert_eq!(params().slow_peer_weight, 0.0);
    }
}
