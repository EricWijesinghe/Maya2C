//! Seal a device seed in a TPM 2.0, bound to PCR state (Linux gateways).
//!
//! Drives `tpm2-tools` rather than linking `tpm2-tss`, so no C library enters
//! any build: `tpm2_createpolicy` for a PCR policy, `tpm2_createprimary` for a
//! storage primary (re-derived identically each time), `tpm2_create` with the
//! seed on stdin as a sealed data object, and `tpm2_load` + `tpm2_unseal` to get
//! it back. A change to a bound PCR — a different bootloader, kernel or measured
//! config — makes unsealing fail, which is the TPM's contribution to tamper
//! evidence. The TPM never signs; see the crate docs.
//!
//! Transient objects and sessions are flushed after every step. Without a
//! resource manager (`/dev/tpmrm0`, `tpm2-abrmd`) each tool leaves them loaded,
//! and a TPM has only a few slots: the third step fails with
//! `TPM_RC_OBJECT_MEMORY`. With one, the flush is a no-op.
//!
//! The TPM is selected by `TPM2TOOLS_TCTI` (e.g. `swtpm:port=2321`).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::device::Seed;
use crate::error::IotError;
use crate::keysource::KeySource;

/// A sealed seed kept as TPM object blobs in a directory.
pub struct TpmSealed {
    dir: PathBuf,
    pcrs: String,
}

fn run(program: &str, args: &[&str], stdin: Option<&[u8]>) -> Result<Vec<u8>, IotError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| IotError::KeySource)?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(input).map_err(|_| IotError::KeySource)?;
    }
    let output = child.wait_with_output().map_err(|_| IotError::KeySource)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(IotError::KeySource)
    }
}

/// Flushes every transient object and loaded session.
fn flush() -> Result<(), IotError> {
    run("tpm2_flushcontext", &["-t"], None)?;
    run("tpm2_flushcontext", &["-s"], None)?;
    Ok(())
}

impl TpmSealed {
    /// A sealed seed in `dir`, bound to `pcrs` (e.g. `sha256:0,7`).
    ///
    /// # Errors
    ///
    /// [`IotError::KeySource`] for a PCR selection with anything but
    /// `[a-z0-9:,]`, so nothing becomes command syntax.
    pub fn new(dir: impl Into<PathBuf>, pcrs: &str) -> Result<Self, IotError> {
        let valid = !pcrs.is_empty()
            && pcrs
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b':' || b == b',');
        if !valid {
            return Err(IotError::KeySource);
        }
        Ok(Self {
            dir: dir.into(),
            pcrs: pcrs.to_owned(),
        })
    }

    fn path(&self, name: &str) -> Result<String, IotError> {
        self.dir
            .join(name)
            .to_str()
            .map(str::to_owned)
            .ok_or(IotError::KeySource)
    }

    fn primary(&self) -> Result<String, IotError> {
        let primary = self.path("primary.ctx")?;
        run("tpm2_createprimary", &["-C", "o", "-c", &primary], None)?;
        flush()?;
        Ok(primary)
    }

    /// Seals `seed` under the current PCR values.
    ///
    /// # Errors
    ///
    /// [`IotError::KeySource`] if any tool fails.
    pub fn seal(&self, seed: &Seed) -> Result<(), IotError> {
        std::fs::create_dir_all(&self.dir).map_err(|_| IotError::KeySource)?;
        let policy = self.path("pcr.policy")?;
        run(
            "tpm2_createpolicy",
            &["--policy-pcr", "-l", &self.pcrs, "-L", &policy],
            None,
        )?;
        flush()?;
        let primary = self.primary()?;
        let (public, private) = (self.path("seal.pub")?, self.path("seal.priv")?);
        run(
            "tpm2_create",
            &[
                "-C", &primary, "-L", &policy, "-i", "-", "-u", &public, "-r", &private,
            ],
            Some(seed.expose()),
        )?;
        flush()
    }

    /// Whether a sealed object exists in the directory.
    #[must_use]
    pub fn is_sealed(&self) -> bool {
        Path::new(&self.dir).join("seal.priv").exists()
    }
}

impl KeySource for TpmSealed {
    fn seed(&mut self) -> Result<Seed, IotError> {
        let primary = self.primary()?;
        let object = self.path("seal.ctx")?;
        let (public, private) = (self.path("seal.pub")?, self.path("seal.priv")?);
        run(
            "tpm2_load",
            &["-C", &primary, "-u", &public, "-r", &private, "-c", &object],
            None,
        )?;
        flush()?;
        let auth = format!("pcr:{}", self.pcrs);
        let unsealed = run("tpm2_unseal", &["-c", &object, "-p", &auth], None);
        flush()?;
        let mut bytes = unsealed?;
        let seed = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| IotError::KeySource);
        bytes.fill(0);
        seed.map(Seed::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pcr_selection_that_could_carry_syntax_is_refused() {
        for bad in ["", "sha256:0 7", "sha256:0;rm", "SHA256:0"] {
            assert!(TpmSealed::new("/tmp/x", bad).is_err(), "{bad:?}");
        }
        assert!(TpmSealed::new("/tmp/x", "sha256:0,7").is_ok());
    }

    /// Needs `swtpm` and `tpm2-tools`, with `TPM2TOOLS_TCTI` pointing at it.
    #[test]
    #[ignore = "needs a TPM (swtpm) and tpm2-tools"]
    fn a_sealed_seed_unseals_and_a_changed_pcr_refuses_it() {
        let dir = std::env::temp_dir().join(format!("maya-iot-tpm-{}", std::process::id()));
        let mut sealed = TpmSealed::new(&dir, "sha256:7").expect("selection");
        let seed = Seed::new([0x5A; 32]);
        sealed.seal(&seed).expect("seal");
        assert!(sealed.is_sealed());
        assert_eq!(sealed.seed().expect("unseal"), seed);

        run(
            "tpm2_pcrextend",
            &["7:sha256=0000000000000000000000000000000000000000000000000000000000000001"],
            None,
        )
        .expect("extend");
        assert_eq!(sealed.seed(), Err(IotError::KeySource));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
