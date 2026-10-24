//! The security council's keys and approvals (ADR-050): 2-of-3 by default,
//! each member's key made and kept on that member's own device.
//!
//! ```text
//! genesis-ceremony council key      --label pc --passphrase-file F [--out-dir D]
//! genesis-ceremony council approve  --key pc.council.sealed --passphrase-file F
//!                                   --council D --genesis G --nonce N
//!                                   (--pause MODULE --blocks N | --resume MODULE)
//! genesis-ceremony council combine  --council D --genesis G APPROVAL...
//! ```
//!
//! `key` writes the secret only sealed (ADR-049: Argon2id + ChaCha20-Poly1305)
//! and the public half as hex. `assemble --council D` puts the public halves in
//! the genesis. `approve` signs one action on one device; `combine` checks the
//! approvals against the council and prints the action for a transaction.
//! Nothing here ever holds two members' secrets at once.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::core::council_payload::{COUNCIL_DOMAIN, CouncilAction, CouncilKind};
use custom_l1_node::crypto::keys::{
    self, PUBLIC_KEY_LEN, SECRET_KEY_LEN, SIGNATURE_LENGTH, SigningKey, VerifyingKey,
};
use custom_l1_node::genesis::{CouncilGenesis, GenesisConfig};
use custom_l1_node::state::invariant_guard::Module;
use maya_signer::sealed;

/// The sealed file's `kind`, so a council key cannot be opened as a validator
/// key or the other way round.
const SEALED_KIND: &str = "council-ml-dsa-65";
const PUBLIC_SUFFIX: &str = ".council.public";
const SEALED_SUFFIX: &str = ".council.sealed";
const APPROVAL_HEADER: &str = "maya2c-council-approval v1";

type Res<T> = Result<T, Box<dyn Error>>;

/// One member as the coordinator sees it: a label and a public key.
pub struct Member {
    pub label: String,
    pub key: [u8; PUBLIC_KEY_LEN],
}

/// The genesis council and its members, in member order.
pub type Council = (CouncilGenesis, Vec<Member>);

/// `genesis-ceremony council ...`.
pub fn run(argv: &[String]) -> Res<()> {
    let (sub, rest) = argv
        .split_first()
        .ok_or("council: key, approve or combine")?;
    let flags = Flags::parse(rest)?;
    match sub.as_str() {
        "key" => make_key(&flags),
        "approve" => approve(&flags),
        "combine" => combine(&flags),
        other => Err(format!("council {other}: expected key, approve or combine").into()),
    }
}

#[derive(Default)]
struct Flags {
    label: String,
    out_dir: Option<PathBuf>,
    out: Option<PathBuf>,
    key: Option<PathBuf>,
    passphrase_file: Option<PathBuf>,
    council: Option<PathBuf>,
    nonce: Option<u64>,
    pause: Option<String>,
    blocks: Option<u64>,
    resume: Option<String>,
    genesis: Option<PathBuf>,
    files: Vec<PathBuf>,
}

impl Flags {
    fn parse(argv: &[String]) -> Res<Self> {
        let mut f = Self::default();
        let mut it = argv.iter();
        while let Some(flag) = it.next() {
            let mut value = || -> Res<String> {
                it.next()
                    .cloned()
                    .ok_or_else(|| format!("{flag} requires a value").into())
            };
            match flag.as_str() {
                "--label" => f.label = value()?,
                "--out-dir" => f.out_dir = Some(value()?.into()),
                "--out" => f.out = Some(value()?.into()),
                "--key" => f.key = Some(value()?.into()),
                "--passphrase-file" => f.passphrase_file = Some(value()?.into()),
                "--council" => f.council = Some(value()?.into()),
                "--nonce" => f.nonce = Some(value()?.parse()?),
                "--pause" => f.pause = Some(value()?),
                "--blocks" => f.blocks = Some(value()?.parse()?),
                "--resume" => f.resume = Some(value()?),
                "--genesis" => f.genesis = Some(value()?.into()),
                other if other.starts_with("--") => {
                    return Err(format!("unknown flag {other}").into());
                }
                file => f.files.push(file.into()),
            }
        }
        Ok(f)
    }

    fn passphrase(&self) -> Res<zeroize::Zeroizing<Vec<u8>>> {
        // A file, never an argument: argv is readable by every process on the
        // machine and lands in shell history (ADR-049).
        let path = self
            .passphrase_file
            .as_ref()
            .ok_or("--passphrase-file is required")?;
        Ok(sealed::read_passphrase_file(path)?)
    }

    /// The chain an approval is for (ADR-036 tag): every approval names it,
    /// so a council key shared by two chains cannot carry one's approvals to
    /// the other.
    fn chain(&self) -> Res<ChainTag> {
        let path = self
            .genesis
            .as_ref()
            .ok_or("--genesis <genesis.json> is required: approvals name their chain")?;
        let config = GenesisConfig::from_json(&fs::read_to_string(path)?)?;
        Ok(ChainTag::from_genesis(config.genesis_block()?.header.id()))
    }

    fn council_dir(&self) -> Res<&Path> {
        Ok(self
            .council
            .as_deref()
            .ok_or("--council <DIR> is required")?)
    }
}

fn make_key(f: &Flags) -> Res<()> {
    super::check_label(&f.label)?;
    let dir = f.out_dir.clone().unwrap_or_else(|| PathBuf::from("."));
    let sealed_path = dir.join(format!("{}{SEALED_SUFFIX}", f.label));
    let public_path = dir.join(format!("{}{PUBLIC_SUFFIX}", f.label));
    if sealed_path.exists() || public_path.exists() {
        return Err(format!(
            "{} already has a council key for {}",
            dir.display(),
            f.label
        )
        .into());
    }
    let passphrase = f.passphrase()?;
    fs::create_dir_all(&dir)?;

    let key = keys::generate_signing_key()?;
    let public = key.verifying_key().to_bytes();
    let text = sealed::seal(key.to_bytes().as_slice(), &public, SEALED_KIND, &passphrase)?;
    super::write_secret(&sealed_path, text.as_bytes())?;
    fs::write(&public_path, hex::encode(public))?;

    println!(
        "Council key written\n\
         ===================\n\
         \n\
         label        {}\n\
         commitment   {}\n\
         \n\
         Send ONLY {} to the coordinator.\n\
         {} is sealed with your passphrase. Keep it, and a copy of the\n\
         passphrase, on this device and nowhere else.\n",
        f.label,
        commitment(&public),
        public_path.display(),
        sealed_path.display(),
    );
    Ok(())
}

/// First 32 hex characters of the BLAKE3 digest: short enough to read aloud.
fn commitment(public: &[u8]) -> String {
    blake3::hash(public).to_hex()[..32].to_string()
}

/// Every `*.council.public` in `dir`, sorted by label: the member order, so
/// member indices depend on the labels, not on the filesystem.
///
/// # Refuses a sealed key
///
/// A coordinator's directory with a member's sealed secret in it means that
/// secret left its device. Sealed is not safe: it is one weak passphrase away.
pub fn load(dir: &Path) -> Res<Vec<Member>> {
    load_members(dir, true)
}

/// As [`load`], but a member signing on their own device may keep their own
/// sealed key beside the public files.
fn load_members(dir: &Path, refuse_sealed: bool) -> Res<Vec<Member>> {
    let mut members = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if refuse_sealed && name.ends_with(SEALED_SUFFIX) {
            return Err(format!(
                "{} contains {name}: a council member's secret left its device. \
                 Make that member a new key on their own device.",
                dir.display()
            )
            .into());
        }
        let Some(label) = name.strip_suffix(PUBLIC_SUFFIX) else {
            continue;
        };
        super::check_label(label)?;
        let bytes = hex::decode(fs::read_to_string(&path)?.trim())
            .map_err(|e| format!("{}: not hex: {e}", path.display()))?;
        let key: [u8; PUBLIC_KEY_LEN] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| format!("{}: not an ML-DSA-65 public key", path.display()))?;
        VerifyingKey::from_bytes(&key)
            .map_err(|e| format!("{}: not an ML-DSA-65 public key: {e}", path.display()))?;
        members.push(Member {
            label: label.to_string(),
            key,
        });
    }
    members.sort_by(|a, b| a.label.cmp(&b.label));
    for pair in members.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(format!("{} and {} are the same key", pair[0].label, pair[1].label).into());
        }
    }
    if members.is_empty() {
        return Err(format!("no *{PUBLIC_SUFFIX} files in {}", dir.display()).into());
    }
    Ok(members)
}

/// The genesis council from `dir`. A zero-length pause and a threshold that
/// is not a strict majority are both refused before a genesis is written.
pub fn genesis(dir: &Path, threshold: u8, max_pause_blocks: u64) -> Res<Council> {
    if max_pause_blocks == 0 {
        return Err("--council-max-pause-blocks must be non-zero".into());
    }
    let members = load(dir)?;
    let council = CouncilGenesis {
        members: members.iter().map(|m| hex::encode(m.key)).collect(),
        threshold,
        max_pause_blocks,
    };
    council.record()?;
    Ok((council, members))
}

/// The commitment sheet's council section.
pub fn sheet_section(council: &CouncilGenesis, members: &[Member]) -> String {
    let lines: Vec<String> = members
        .iter()
        .enumerate()
        .map(|(i, m)| format!("  member {i}  {:<16} {}", m.label, commitment(&m.key)))
        .collect();
    format!(
        "\nSecurity council: {} of {}, a pause lasts at most {} blocks\n{}\n",
        council.threshold,
        members.len(),
        council.max_pause_blocks,
        lines.join("\n")
    )
}

fn module(name: &str) -> Res<u8> {
    (0..=u8::MAX)
        .find(|t| Module::from_tag(*t).is_some_and(|m| format!("{m:?}").eq_ignore_ascii_case(name)))
        .ok_or_else(|| format!("unknown module {name}").into())
}

fn requested_kind(f: &Flags) -> Res<CouncilKind> {
    match (&f.pause, &f.resume) {
        (Some(m), None) => Ok(CouncilKind::Pause {
            module: module(m)?,
            blocks: f.blocks.ok_or("--pause needs --blocks")?,
        }),
        (None, Some(m)) if f.blocks.is_none() => Ok(CouncilKind::Resume { module: module(m)? }),
        _ => Err("exactly one of --pause MODULE --blocks N or --resume MODULE".into()),
    }
}

fn approve(f: &Flags) -> Res<()> {
    let members = load_members(f.council_dir()?, false)?;
    let key_path = f.key.as_ref().ok_or("--key is required")?;
    let (secret, public) = sealed::open(
        &fs::read_to_string(key_path)?,
        SEALED_KIND,
        &f.passphrase()?,
    )?;
    let secret: &[u8; SECRET_KEY_LEN] = secret
        .as_slice()
        .try_into()
        .map_err(|_| "sealed key: length")?;
    let key = SigningKey::from_bytes(secret)?;
    if key.verifying_key().to_bytes().as_slice() != public.as_slice() {
        return Err("sealed key: the public key does not match the secret".into());
    }
    let index = members
        .iter()
        .position(|m| m.key.as_slice() == public.as_slice())
        .ok_or("this key is not a member of that council")?;
    let action = CouncilAction {
        kind: requested_kind(f)?,
        nonce: f
            .nonce
            .ok_or("--nonce is required: the council's current nonce")?,
        approvals: Vec::new(),
    };
    let message = action.signing_message(&f.chain()?);
    let text = format!(
        "{APPROVAL_HEADER}\nmember {index}\naction {}\nsignature {}\n",
        hex::encode(&message),
        hex::encode(key.sign(&message)?)
    );
    let out = f
        .out
        .clone()
        .unwrap_or_else(|| format!("{}.approval", members[index].label).into());
    fs::write(&out, text)?;
    println!(
        "{:?} nonce {} approved by member {index} ({}): {}",
        action.kind,
        action.nonce,
        members[index].label,
        out.display()
    );
    Ok(())
}

/// One approval file: member index, the signed message, the signature.
fn read_approval(path: &Path) -> Res<(u8, Vec<u8>, Box<[u8; SIGNATURE_LENGTH]>)> {
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines();
    if lines.next() != Some(APPROVAL_HEADER) {
        return Err(format!("{}: not a council approval", path.display()).into());
    }
    let mut field = |name: &str| -> Res<String> {
        lines
            .next()
            .and_then(|l| l.strip_prefix(name))
            .map(|v| v.trim().to_string())
            .ok_or_else(|| format!("{}: missing {name}", path.display()).into())
    };
    let member = field("member ")?.parse()?;
    let message = hex::decode(field("action ")?)?;
    let sig: [u8; SIGNATURE_LENGTH] = hex::decode(field("signature ")?)?
        .as_slice()
        .try_into()
        .map_err(|_| format!("{}: signature length", path.display()))?;
    Ok((member, message, Box::new(sig)))
}

/// Joins approvals into one action. Every approval must sign the same
/// message, come from a distinct member, and verify; at least a strict
/// majority must remain, because a chain with fewer will refuse it anyway.
pub fn combine_approvals(
    members: &[Member],
    chain: &ChainTag,
    approvals: Vec<(u8, Vec<u8>, Box<[u8; SIGNATURE_LENGTH]>)>,
) -> Res<CouncilAction> {
    let message = approvals
        .first()
        .map(|a| a.1.clone())
        .ok_or("no approvals given")?;
    let mut seen = BTreeSet::new();
    let mut signed = Vec::new();
    for (member, m, sig) in approvals {
        if m != message {
            return Err(format!("member {member} approved a different action").into());
        }
        let key = members
            .get(usize::from(member))
            .ok_or(format!("no member {member}"))?;
        VerifyingKey::from_bytes(&key.key)?
            .verify(&message, &sig)
            .map_err(|_| format!("member {member} ({}): signature does not verify", key.label))?;
        if !seen.insert(member) {
            return Err(format!("member {member} approved twice").into());
        }
        signed.push((member, sig));
    }
    if signed.len() * 2 <= members.len() {
        return Err(format!(
            "{} approvals of {} members is not a majority",
            signed.len(),
            members.len()
        )
        .into());
    }
    // The message is the domain, the chain tag and the action without its
    // approvals; with a zero approval count appended it decodes as an action.
    let body = message
        .strip_prefix(COUNCIL_DOMAIN)
        .ok_or("not a council message (or one from before the v2 domain)")?
        .strip_prefix(chain.as_bytes().as_slice())
        .ok_or("these approvals are for another chain")?;
    let mut bytes = body.to_vec();
    bytes.extend_from_slice(&0u64.to_le_bytes());
    let mut reader = ByteReader::new(&bytes);
    let mut action = CouncilAction::decode(&mut reader)?;
    reader.finish()?;
    // What the chain will rebuild and check must be what was signed;
    // otherwise the transaction would only fail on chain.
    if action.signing_message(chain) != message {
        return Err("the approved message is not a canonical council action".into());
    }
    action.approvals = signed;
    Ok(action)
}

fn combine(f: &Flags) -> Res<()> {
    let members = load(f.council_dir()?)?;
    let approvals = f
        .files
        .iter()
        .map(|p| read_approval(p))
        .collect::<Res<Vec<_>>>()?;
    let action = combine_approvals(&members, &f.chain()?, approvals)?;
    let mut encoded = Vec::new();
    action.encode_into(&mut encoded);
    let out = f
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("council-action.hex"));
    fs::write(&out, hex::encode(&encoded))?;
    println!(
        "{:?} nonce {} with {} approvals: {}",
        action.kind,
        action.nonce,
        action.approvals.len(),
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn member(i: u8) -> (SigningKey, Member) {
        let k = keys::signing_key_from_seed(&[0x70 + i; 32]).unwrap();
        let m = Member {
            label: format!("m{i}"),
            key: k.verifying_key().to_bytes(),
        };
        (k, m)
    }

    fn approval(
        k: &SigningKey,
        i: u8,
        kind: CouncilKind,
    ) -> (u8, Vec<u8>, Box<[u8; SIGNATURE_LENGTH]>) {
        let msg = CouncilAction {
            kind,
            nonce: 7,
            approvals: vec![],
        }
        .signing_message(&TEST_CHAIN);
        let sig = k.sign(&msg).unwrap();
        (i, msg, Box::new(sig))
    }

    const TEST_CHAIN: ChainTag = ChainTag::from_genesis([0x42; 32]);

    const PAUSE: CouncilKind = CouncilKind::Pause {
        module: 1,
        blocks: 60,
    };

    #[test]
    fn two_of_three_combine_into_an_action_that_round_trips() {
        let ms: Vec<_> = (0..3).map(member).collect();
        let council: Vec<Member> = ms
            .iter()
            .map(|(_, m)| Member {
                label: m.label.clone(),
                key: m.key,
            })
            .collect();
        let action = combine_approvals(
            &council,
            &TEST_CHAIN,
            vec![approval(&ms[0].0, 0, PAUSE), approval(&ms[2].0, 2, PAUSE)],
        )
        .unwrap();
        assert_eq!(
            (action.kind, action.nonce, action.approvals.len()),
            (PAUSE, 7, 2)
        );
        let mut bytes = Vec::new();
        action.encode_into(&mut bytes);
        assert_eq!(
            CouncilAction::decode(&mut ByteReader::new(&bytes)).unwrap(),
            action
        );
    }

    #[test]
    fn one_approval_a_repeat_a_forgery_or_a_different_action_is_refused() {
        let ms: Vec<_> = (0..3).map(member).collect();
        let council: Vec<Member> = ms
            .iter()
            .map(|(_, m)| Member {
                label: m.label.clone(),
                key: m.key,
            })
            .collect();
        let a0 = || approval(&ms[0].0, 0, PAUSE);
        assert!(combine_approvals(&council, &TEST_CHAIN, vec![a0()]).is_err());
        assert!(combine_approvals(&council, &TEST_CHAIN, vec![a0(), a0()]).is_err());
        // Member 0's key claiming to be member 1.
        assert!(
            combine_approvals(
                &council,
                &TEST_CHAIN,
                vec![a0(), approval(&ms[0].0, 1, PAUSE)]
            )
            .is_err()
        );
        let resume = CouncilKind::Resume { module: 1 };
        assert!(
            combine_approvals(
                &council,
                &TEST_CHAIN,
                vec![a0(), approval(&ms[1].0, 1, resume)]
            )
            .is_err()
        );
    }

    #[test]
    fn a_message_with_trailing_bytes_is_refused() {
        let (k, m) = member(0);
        let (k1, m1) = member(1);
        let council = vec![m, m1];
        let mut msg = CouncilAction {
            kind: PAUSE,
            nonce: 7,
            approvals: vec![],
        }
        .signing_message(&TEST_CHAIN);
        msg.push(0);
        let sign = |k: &SigningKey, i| (i, msg.clone(), Box::new(k.sign(&msg).unwrap()));
        assert!(combine_approvals(&council, &TEST_CHAIN, vec![sign(&k, 0), sign(&k1, 1)]).is_err());
    }

    #[test]
    fn approvals_for_another_chain_do_not_combine() {
        let ms: Vec<_> = (0..3).map(member).collect();
        let council: Vec<Member> = ms
            .iter()
            .map(|(_, m)| Member {
                label: m.label.clone(),
                key: m.key,
            })
            .collect();
        let a = [approval(&ms[0].0, 0, PAUSE), approval(&ms[1].0, 1, PAUSE)];
        let elsewhere = ChainTag::from_genesis([0x43; 32]);
        assert!(combine_approvals(&council, &elsewhere, a.to_vec()).is_err());
        assert!(combine_approvals(&council, &TEST_CHAIN, a.to_vec()).is_ok());
    }

    #[test]
    fn module_names_map_to_their_tags() {
        assert_eq!(module("dex").unwrap(), 1);
        assert_eq!(module("Vm").unwrap(), 5);
        assert!(module("everything").is_err());
    }
}
