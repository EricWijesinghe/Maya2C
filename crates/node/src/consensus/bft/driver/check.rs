//! The check gossip waits for before it relays a DAG-BFT frame.
//!
//! A frame used to be relayed once it decoded, so a forged one reached every
//! node and was blamed on whoever signed the gossip message, which costs
//! nothing to vary (audit P2P-N1 follow-up). Gossip now holds its verdict
//! until this check says every signature the frame carries verifies, so a
//! forgery stops at the first node that receives it.
//!
//! The check reads nothing but the frame and this epoch's committee, never
//! the engine's state: an observer or a node that already holds a
//! certificate relays it on the same terms as a validator seeing it for the
//! first time, so honest relays agree with each other on what they forward.

use maya_dag_bft::{Authenticator, Message};

use super::{ATTEST_FRAME, Attestation, BftDriver, Envelope};
use crate::network::FrameCheck;

impl BftDriver {
    /// Whether `bytes`, a frame gossip delivered, may be relayed. Changes
    /// nothing and needs no chain lock, so the binary runs it before taking
    /// one.
    #[must_use]
    pub fn check_frame(&self, bytes: &[u8]) -> FrameCheck {
        if self.halted {
            return FrameCheck::Unjudged;
        }
        if bytes.first() == Some(&ATTEST_FRAME) {
            return self.check_attestation(bytes);
        }
        // Gossip passes on only frames that decode, so this is defensive.
        let Ok(envelope) = Envelope::decode(bytes) else {
            return FrameCheck::Forged;
        };
        if envelope.epoch != self.epoch {
            return FrameCheck::Unjudged;
        }
        let auth = &self.checker;
        let authentic = match &envelope.message {
            Message::Propose { vertex, signature } => {
                auth.verify(vertex.author, &vertex.digest(), signature)
            }
            Message::Vote {
                digest,
                voter,
                signature,
                ..
            } => auth.verify(*voter, digest, signature),
            Message::Cert(cert) => {
                // Well formed first: an empty vote list verifies vacuously.
                cert.is_well_formed(&self.checked_committee) && {
                    let digest = cert.digest();
                    cert.votes
                        .iter()
                        .zip(&cert.signatures)
                        .all(|(voter, signature)| auth.verify(*voter, &digest, signature))
                }
            }
            // A fetch carries no signature; its sender's id is all there is.
            Message::Fetch(_) => envelope.from < self.checked_committee.size(),
        };
        if authentic {
            FrameCheck::Authentic
        } else {
            FrameCheck::Forged
        }
    }

    /// An attestation of this epoch or the one just closed, checked against
    /// that epoch's committee; any other epoch is not judged.
    fn check_attestation(&self, bytes: &[u8]) -> FrameCheck {
        let Ok(attestation) = Attestation::decode(bytes) else {
            return FrameCheck::Forged;
        };
        let keys = if attestation.epoch == self.epoch {
            &self.committee
        } else if let Some(closing) = self
            .closing
            .as_ref()
            .filter(|c| c.epoch == attestation.epoch)
        {
            &closing.keys
        } else {
            return FrameCheck::Unjudged;
        };
        if attestation.verify(&self.tag, keys).is_ok() {
            FrameCheck::Authentic
        } else {
            FrameCheck::Forged
        }
    }
}
