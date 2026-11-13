//! Council key rotation (ADR-082 amendment, 2026-11-14): each new member
//! accepts its seat on its own device, then the council in force approves the
//! rotation with the acceptances in it.
//!
//! ```text
//! genesis-ceremony council accept   --key new.council.sealed --passphrase-file F
//!                                   --genesis G --rotate NEW_DIR
//!                                   --threshold T --activation H
//! genesis-ceremony council approve  ... --nonce N --rotate NEW_DIR
//!                                   --threshold T --activation H ACCEPTANCE...
//! ```
//!
//! `NEW_DIR` holds the new members' `*.council.public` files; their member
//! order is its label order, as for the genesis council. Once the rotation
//! has taken over, `NEW_DIR` *is* the council: pass it as `--council` to
//! `approve` and `combine`, or member indices name the old set's keys.

use std::fs;
use std::path::{Path, PathBuf};

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::council_payload::Rotation;
use custom_l1_node::crypto::keys::{SIGNATURE_LENGTH, VerifyingKey};
use custom_l1_node::state::council::check_member_set;

use super::council::{Member, Res};

pub const ACCEPTANCE_HEADER: &str = "maya2c-council-acceptance v1";

/// The rotation `--rotate`, `--threshold` and `--activation` name, and the
/// new members by label. Refused here if the chain would refuse the set.
pub fn requested(
    members: Vec<Member>,
    threshold: Option<u8>,
    activation: Option<u64>,
) -> Res<(Rotation, Vec<Member>)> {
    let rotation = Rotation {
        activation: activation.ok_or("--rotate needs --activation H")?,
        threshold: threshold.ok_or("--rotate needs --threshold T")?,
        members: members.iter().map(|m| m.key).collect(),
    };
    check_member_set(rotation.threshold, &rotation.members)?;
    Ok((rotation, members))
}

/// The acceptance file text for new member `index`.
pub fn acceptance_text(index: usize, message: &[u8], sig: &[u8]) -> String {
    format!(
        "{ACCEPTANCE_HEADER}\nmember {index}\nrotation {}\nsignature {}\n",
        hex::encode(message),
        hex::encode(sig)
    )
}

/// One acceptance file: new member index, the signed message, the signature.
fn read_acceptance(path: &Path) -> Res<(u8, Vec<u8>, Box<[u8; SIGNATURE_LENGTH]>)> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    if lines.next() != Some(ACCEPTANCE_HEADER) {
        return Err(format!("{}: not a council acceptance", path.display()).into());
    }
    let mut field = |name: &str| -> Res<String> {
        lines
            .next()
            .and_then(|l| l.strip_prefix(name))
            .map(|v| v.trim().to_string())
            .ok_or_else(|| format!("{}: missing {name}", path.display()).into())
    };
    let member = field("member ")?.parse()?;
    let message = hex::decode(field("rotation ")?)?;
    let sig: [u8; SIGNATURE_LENGTH] = hex::decode(field("signature ")?)?
        .as_slice()
        .try_into()
        .map_err(|_| format!("{}: signature length", path.display()))?;
    Ok((member, message, Box::new(sig)))
}

/// Every new member's acceptance, in member order: each must accept exactly
/// this rotation on this chain, verify, and appear once.
#[allow(clippy::vec_box)] // the type of `CouncilKind::Rotate::acceptances`
pub fn acceptances(
    rotation: &Rotation,
    chain: &ChainTag,
    signed: Vec<(u8, Vec<u8>, Box<[u8; SIGNATURE_LENGTH]>)>,
) -> Res<Vec<Box<[u8; SIGNATURE_LENGTH]>>> {
    let message = rotation.acceptance_message(chain);
    let mut slots: Vec<Option<Box<[u8; SIGNATURE_LENGTH]>>> = vec![None; rotation.members.len()];
    for (member, m, sig) in signed {
        if m != message {
            return Err(format!(
                "new member {member} accepted another rotation, height, threshold or chain"
            )
            .into());
        }
        let key = rotation
            .members
            .get(usize::from(member))
            .ok_or(format!("no new member {member}"))?;
        VerifyingKey::from_bytes(key)?
            .verify(&message, &sig)
            .map_err(|_| format!("new member {member}: acceptance does not verify"))?;
        let slot = &mut slots[usize::from(member)];
        if slot.is_some() {
            return Err(format!("new member {member} accepted twice").into());
        }
        *slot = Some(sig);
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(i, s)| s.ok_or_else(|| format!("no acceptance from new member {i}").into()))
        .collect()
}

/// [`acceptances`] from files.
#[allow(clippy::vec_box)] // the type of `CouncilKind::Rotate::acceptances`
pub fn acceptances_from(
    rotation: &Rotation,
    chain: &ChainTag,
    files: &[PathBuf],
) -> Res<Vec<Box<[u8; SIGNATURE_LENGTH]>>> {
    let signed = files
        .iter()
        .map(|p| read_acceptance(p))
        .collect::<Res<Vec<_>>>()?;
    acceptances(rotation, chain, signed)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use custom_l1_node::crypto::keys::{self, SigningKey};

    const CHAIN: ChainTag = ChainTag::from_genesis([0x42; 32]);

    fn new_set() -> (Vec<SigningKey>, Rotation) {
        let ks: Vec<_> = (0..3u8)
            .map(|i| keys::signing_key_from_seed(&[0x90 + i; 32]).unwrap())
            .collect();
        let members = ks
            .iter()
            .enumerate()
            .map(|(i, k)| Member {
                label: format!("n{i}"),
                key: k.verifying_key().to_bytes(),
            })
            .collect();
        let (rotation, _) = requested(members, Some(2), Some(500)).unwrap();
        (ks, rotation)
    }

    fn accept(
        k: &SigningKey,
        i: u8,
        r: &Rotation,
        chain: &ChainTag,
    ) -> (u8, Vec<u8>, Box<[u8; SIGNATURE_LENGTH]>) {
        let m = r.acceptance_message(chain);
        let sig = Box::new(k.sign(&m).unwrap());
        (i, m, sig)
    }

    #[test]
    fn every_new_member_must_accept_once_for_this_rotation() {
        let (ks, r) = new_set();
        let all: Vec<_> = [2u8, 0, 1]
            .iter()
            .map(|i| accept(&ks[usize::from(*i)], *i, &r, &CHAIN))
            .collect();
        let sigs = acceptances(&r, &CHAIN, all.clone()).unwrap();
        // Back in member order whatever order the files came in.
        let message = r.acceptance_message(&CHAIN);
        for (key, sig) in r.members.iter().zip(&sigs) {
            VerifyingKey::from_bytes(key)
                .unwrap()
                .verify(&message, sig)
                .unwrap();
        }
        // One missing, one twice, one by the wrong key, one for another chain.
        assert!(acceptances(&r, &CHAIN, all[..2].to_vec()).is_err());
        let mut twice = all.clone();
        twice.push(all[0].clone());
        assert!(acceptances(&r, &CHAIN, twice).is_err());
        let mut forged = all.clone();
        forged[0] = accept(&ks[0], 2, &r, &CHAIN);
        assert!(acceptances(&r, &CHAIN, forged).is_err());
        let mut elsewhere = all;
        elsewhere[0] = accept(&ks[2], 2, &r, &ChainTag::from_genesis([0xEE; 32]));
        assert!(acceptances(&r, &CHAIN, elsewhere).is_err());
    }

    #[test]
    fn a_set_the_chain_would_refuse_is_refused_before_anyone_signs() {
        let (_, r) = new_set();
        let members = |n: usize| {
            r.members[..n]
                .iter()
                .map(|key| Member {
                    label: String::new(),
                    key: *key,
                })
                .collect::<Vec<_>>()
        };
        // 1 of 2 is not a strict majority; no threshold or height given.
        assert!(requested(members(2), Some(1), Some(500)).is_err());
        assert!(requested(members(3), None, Some(500)).is_err());
        assert!(requested(members(3), Some(2), None).is_err());
    }
}
