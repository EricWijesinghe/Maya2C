# ADR-049: Sealed validator keys at rest

**Status:** Accepted, 2026-10-18

> **Superseded in part (doc audit 2026-11-03):** validator consensus keys are ML-DSA-87, not ML-DSA-65 (ADR-051 option 3; PR #222; `crates/node/src/crypto/validator_keys.rs`, 2,592-byte public key). The sealed key kind is `maya2c-ml-dsa-87-validator-seed`, a 32-byte seed (`crates/signer/src/sealed.rs`). ML-DSA-65 below describes the original 2026-10-18 design; the sealing mechanism is unchanged.
**Context owner:** Eric Wijesinghe (Phase-1 security plan, approved 2026-10-17)

## Context

An audit on 2026-10-17 found two problems on the live testnet:

- Each of the four validators keeps its ML-DSA-65 consensus key as **plaintext hex**.
- Every local account could read those files, and BitLocker is off on both volumes.

Anyone who copied one file could sign as that validator. Permissions were tightened by hand on the same day, but permissions do not survive:

- a copied backup,
- a disk taken out of the machine,
- a mistaken `icacls`.

`crates/signer/src/keystore.rs` already seals a 32-byte master seed (ADR-022). A node's validator key is the *expanded* 4,032-byte ML-DSA-65 secret key, written by `--generate-validator-key`. It is not derived from a seed, so existing keys cannot be moved into that format.

## Decision

1. **A second sealed format, `crates/signer/src/sealed.rs`.**
   - It seals any secret byte string with the keystore's primitives:
     - Argon2id, 64 MiB, 3 passes;
     - ChaCha20-Poly1305, with a random 16-byte salt and 12-byte nonce.
   - Every cleartext field is bound in as length-prefixed associated data: format, kind, KDF parameters, salt and public key. So swapping the public key, cheapening the KDF or relabelling the kind fails the tag, exactly like a wrong passphrase does.
   - Two bounds:
     - KDF parameters outside 8–256 MiB or 1–10 passes are refused *before* hashing, so a crafted file can neither stall the host nor trip the KDF's internal errors.
     - Passphrases under 12 bytes are refused when sealing.
2. **The node reads the passphrase from a file.** It comes from `--validator-passphrase-file` or from the file named by `MAYA2C_VALIDATOR_PASSPHRASE_FILE`. Never from argv or an environment value: both are visible to any process that can list this one.
3. **New and migrated keys.**
   - `--generate-validator-key` seals when a passphrase file is set.
   - `--seal-validator-key <IN> <OUT>` migrates an existing key. It refuses to overwrite `OUT`, and it reopens `OUT` and compares public keys before reporting success.
   - **Key files are written atomically.** The file goes to `<OUT>.partial-<pid>`, is verified there, and is then hard-linked into place, which fails rather than replace a file that appeared in the meantime. A crash or a failed check therefore leaves no partial key behind to block a retry.
   - **Flags may come in any order.** Both commands run only after every flag is parsed, so a passphrase flag placed after the command still applies. Before this, it would have silently produced a plaintext key.
4. **Plaintext keys:**
   - **A `production` build refuses them,** both when loading and when generating. A mainnet validator cannot start with a readable consensus key.
   - **Upgrade note:** a validator moved to a `--features production` binary will not start until its key is sealed.
   - **Devnet and testnet builds still load them,** with a warning, so the running testnet is not broken by an upgrade.
5. **The wallet** (`l1-wallet register-validator`) opens both formats, using the same kind label and passphrase variable, and warns on a plaintext key.
6. **On Windows there is no 0600.** The key file inherits its directory's ACL, so the key directory must be restricted to the service account; see the 2026-10-17 audit.

## Consequences

- **Moving the testnet:** each operator runs `--seal-validator-key`, restarts with the passphrase flag, and then destroys the plaintext file. That is a restart of each validator, done one at a time and never during a halt.
- **The passphrase file is the new secret.**
  - It belongs **off the key's volume** or in OS-protected storage.
  - Next step: Windows DPAPI or a TPM-sealed passphrase. Until then, a passphrase file stored next to the key only protects against copies of the key alone.
- **Not covered:**
  - **Memory.** The key is in process memory while the node runs; zeroized buffers limit stray copies but do not stop a debugger. The remote signer (ADR-033) remains the stronger option for a validator that can run one.
  - **No external review.** The format has unit tests for every tamper case, but it has not been externally reviewed.

## Review

On 2026-10-18, a rust-reviewer pass and a security-reviewer pass were run by AI agents. These are not independent human reviews. Fixed from them:

- flag-order plaintext;
- production plaintext generation;
- non-atomic writes;
- loose KDF bounds;
- newline stripping;
- missing tamper tests.

## Tests

- `crates/signer/src/sealed.rs` covers:
  - round trip;
  - the secret absent from the file;
  - wrong passphrase;
  - swapped public key;
  - weakened KDF;
  - flipped ciphertext bit;
  - wrong kind;
  - KDF parameters out of bounds;
  - swapped salt;
  - passphrase-file newline handling;
  - short passphrase.
- `bins/maya2c-node/src/validator_key.rs` covers:
  - generate and load;
  - sealed key without a passphrase;
  - wrong passphrase;
  - seal preserving the key and refusing overwrite;
  - CRLF passphrase files;
  - devnet plaintext;
  - production refusal of loading and of generating plaintext;
  - mismatched public key;
  - no file left after a failed seal.
- `bins/l1-wallet/src/staking.rs` covers a sealed key loading only with its passphrase.

## Amendment 2026-11-18: passphrase files and keystores are owner-only

Pre-approved by Eric on 2026-11-17 into re-freeze bundle 3 (PR #596).

- **Passphrase files.** `sealed::read_passphrase_file` refuses, on Unix, a file
  its group or others can read (mode must be 0600 or tighter). The mode is read
  from the open handle, so the file checked is the file read. Every reader goes
  through it: the node's validator key, the genesis-ceremony council key, the
  wallet's staking path and the `maya2c-signer` CLI. This is a hard refusal, not
  a warning: a passphrase another local user can read protects nothing. The
  installers (`infra/testnet-vm/install.sh`, `scripts/join-testnet.sh`) already
  write 0600; the dev and harness writers (`maya2c-cli dev`, `xtask` localnet,
  ceremony rehearsal, proving) and the release workflow's smoke test
  (`.github/workflows/release-binaries.yml`, under `umask 077`) now do too.
  Operators on macOS or Linux create the file with `umask 077` or `chmod 600`
  on a filesystem that keeps Unix permissions; FAT, exFAT and NTFS removable
  media cannot hold it (runbooks: `genesis-ceremony.md`,
  `regenesis-checklist.md`, `node-in-a-box-linux.md`).
- **Windows.** ACLs are not inspected; point 6 above still applies. (Superseded by the 2026-11-24 amendment below.)
- **Remote-signer keystores** (`keystore::create`) refuse a passphrase shorter
  than `MIN_PASSPHRASE_LEN` (12 bytes, as sealing already did), refuse to
  overwrite an existing file, and create the file 0600 on Unix.
  `keystore::open` caps the file at 64 KiB before parsing, applies the sealed
  format's KDF bounds, and refuses any non-hex character instead of panicking.
  A write or sync failure during `create` removes the partial file, so a retry
  is not refused as an existing keystore.
- **Slashing-protection database.** Compaction opens the new file's append
  handle before the rename; import refuses an author at or above
  `MAX_AUTHORS`, as signing does.

Tests: `crates/signer/tests/hardening_tests.rs`, and
`init_refuses_a_passphrase_file_others_can_read` in
`bins/maya2c-signer/tests/cli_tests.rs`.

## Amendment 2026-11-24: owner-only key and passphrase files on Windows (audit NEW-01)

Point 6 left Windows key files to their folder's ACL, and mainnet validators
run on a Windows PC (2026-10-11 decision). Now:

- **New key files** (`maya2c-node --generate-validator-key` and
  `--seal-validator-key`, `maya2c-signer init`) are opened unshared and their
  inherited DACL is replaced with one full-control entry for the account
  running the process before any byte is written
  (`maya_signer::owner_only::restrict`, `icacls /inheritance:r /grant:r`).
- **Passphrase files** are refused when their DACL grants any account other
  than the account running the node, SYSTEM or Administrators
  (`owner_only::check`, the Hub key check's allow list). Deny and inherit-only
  entries are ignored; an entry the parser cannot read, a null DACL and a
  missing DACL are refused. Like the Unix rule, this is a hard error. The
  message gives the `icacls` command that fixes the file, for the running
  account's SID. "The account running the node" matters: the boot task that
  `cargo xtask node setup` prints runs the node as SYSTEM, so the operator's
  own grant counts as another account. Setup therefore grants the key files
  to SYSTEM only and removes the operator's grant (the operator stays owner);
  `node doctor` checks against SYSTEM too. `maya2c dev` and the `xtask` devnet
  writers grant the account that runs their nodes. Test:
  `setup_acl_passes_the_node_check_when_the_node_runs_as_system`
  (`xtask/src/node_box/acl.rs`, Windows only).
- **Upgrading a live Windows validator to this rule:** before the new binary
  starts, list every passphrase file's ACL (`icacls <file>`) and remove any
  grant other than the account the node runs as, SYSTEM and Administrators —
  an entry inherited from the drive root (Users, Authenticated Users) is
  enough to stop the node. Runbook: `docs/runbooks/validator-start-guards.md`.
- **How:** the system's own `whoami`, `icacls` and Windows PowerShell
  (`Get-Acl`, path passed in an environment variable), by full path under
  `%SystemRoot%`, so no Windows security API dependency enters a crate the
  node links. If they cannot run, the read fails closed.
- **Not covered:** existing key files keep their ACL (run `cargo xtask node
  setup` or the `icacls` command once). The DACL is read from the path, not
  the open handle.

Tests: `a_new_keystore_in_a_folder_everyone_can_read_is_owner_only` and
`a_passphrase_file_everyone_can_read_is_refused_on_windows`
(`crates/signer/tests/hardening_tests.rs`),
`a_new_key_in_a_folder_everyone_can_read_is_owner_only`
(`bins/maya2c-node/src/validator_key.rs`), and the SDDL parser tests in
`crates/signer/src/owner_only.rs`. The Windows tests run only on Windows; no
CI test job runs these crates on Windows.
