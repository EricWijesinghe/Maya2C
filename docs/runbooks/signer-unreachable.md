# Runbook: Remote signer unreachable

**Symptom.** Validator cannot sign; `maya_validator_missed_rounds` rising (not emitted yet).

**Check.** Signer process up? Channel handshake errors (`peer identity is not pinned` means a key mismatch)?

**Act.** Restore the signer on its dedicated host. **Never** start a second signer with the same key without importing the first one's slashing-protection history (interchange file).

**Rehearsal.** rehearsed in tests: `crates/signer/tests/signer_tests.rs` (pinning, restart from backup refused)
