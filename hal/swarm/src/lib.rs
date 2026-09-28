//! Robot swarms on a ledger (Master Prompt 6 §8): the wire formats robots
//! speak, task micro-auctions, and collision avoidance checked off chain
//! against plans committed on chain.
//!
//! - [`mavlink`] — `MAVLink` 2 framing, HEARTBEAT and `GLOBAL_POSITION_INT`
//!   (REAL: checked byte for byte against pymavlink).
//! - [`cdr`] — ROS 2's CDR serialization of `Pose` and `PoseStamped` (REAL:
//!   checked against the rosbags serializer).
//! - [`auction`] — sealed-bid task auctions.
//! - [`paths`] — committed flight plans and provable conflicts.
//!
//! RESEARCH: nothing in the node calls this crate, and no robot has flown
//! with it. The swarm in its tests is simulated.

pub mod auction;
pub mod cdr;
pub mod mavlink;
pub mod paths;

/// An agent (robot) id.
pub type AgentId = u32;
/// A task id.
pub type TaskId = u32;

/// Why a frame or an action was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SwarmError {
    /// Bytes that contradict the format.
    #[error("malformed: {0}")]
    Malformed(&'static str),
    /// Valid, but outside what this crate decodes.
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    /// An auction or plan rule refused it.
    #[error("refused: {0}")]
    Refused(&'static str),
}
