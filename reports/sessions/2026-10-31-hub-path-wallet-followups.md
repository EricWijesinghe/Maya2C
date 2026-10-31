# 2026-10-31 Hub miner pinning and wallet signing follow-ups

Branch `fix/hub-miner-path-wallet-followups`, based on `feat/clear-sign-screens` (PR #271, not merged).
Threat model: `docs/security/THREAT_MODEL_SIGNER_CHAIN.md` P5 (miner binary and key file from Hub settings).

## A. Hub: miner binary and key file (P5)

Changes: `apps/hub/src-tauri/src/miner_guard.rs` (new), `miner.rs`, `lib.rs`, `build.rs`, `Cargo.toml` (`sha2 = "0.10"`, already in `Cargo.lock` at 0.10.9; the lock diff is one line).

- Miner resolves only to `maya2c-gpu-miner[.exe]` in the Hub's install folder. A bare name is taken from there; a path must canonicalize into it. A symlink or junction that leaves the folder is refused.
- The miner's SHA-256 must match a `sha256sum` line in `MAYA2C_MINER_SHA256SUMS`, embedded at build time by `build.rs`. Unset means no miner runs (fail closed).
- Key file must canonicalize inside the Hub data folder, including a symlink target.
- Errors are a typed `GuardError` (`Display` text shown in the GUI via the `miner_start` command).

Test output (`cargo test --manifest-path apps/hub/src-tauri/Cargo.toml`, `CARGO_BUILD_JOBS=1`):

```
running 21 tests
test miner_guard::tests::symlink_escape_out_of_the_install_folder_is_refused ... ok
test miner_guard::tests::path_traversal_out_of_the_install_folder_is_refused ... ok
test miner_guard::tests::absolute_path_outside_the_install_folder_is_refused ... ok
test miner_guard::tests::hash_mismatch_is_refused ... ok
test miner_guard::tests::bundled_miner_with_a_matching_hash_is_accepted ... ok
test miner_guard::tests::key_file_path_out_of_the_data_folder_is_refused ... ok
test miner_guard::tests::key_file_symlink_escape_is_refused ... ok
test miner_guard::tests::key_file_inside_the_data_folder_is_accepted ... ok
test miner_guard::tests::miner_absent_from_the_checksums_is_refused ... ok
test miner_guard::tests::unknown_program_name_is_refused ... ok
test result: ok. 21 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.04s
```

The symlink tests passed on this Windows machine, so symlink creation worked here.

## B. Wallet core (PR #271 follow-ups)

1. Unbound signing removed. `payment::sign_transfer`, `payment::sign_transfer_to`, `sign_with_fee_to` and the `compose_swap` export are gone. The only signing entry is `Wallet::sign`, which requires the confirm hash. `compose_swap` is `#[cfg(test)]` with one test. `transfer_size` now signs a probe via `finish_transfer` (never broadcast). `examples/devnet_send.rs` signs via `Wallet::confirm` then `Wallet::sign`. Integration tests call a local `sign_transfer` helper that routes through the same gate and asserts the key is account 0.
2. Typed recipients are checksum-checked. `normalize_address` refuses mixed-case input that does not match the EIP-55-style checksum. All-lower and all-upper input carry no claim and are accepted.
3. `describe` labels fee outputs. Output 0 is always the payment. Later outputs are fees only when they pay `FEE_COLLECTOR` or the burn sink `FEE_SINK`; any other extra output is still a recipient. Summary: `Send 1,000 base units to 1 recipient, with a fee of 7 base units.` `confirm.rs` uses the same `is_fee_output` for its fee field.

Test output, `cargo test -p maya-wallet-core` (`CARGO_BUILD_JOBS=1`, full run, file `D:/Temp/claude/D--Maya2C/b2f76622-4adf-46bc-a177-0004f34f213a/scratchpad_wallet_full.txt`):

```
Running unittests src\lib.rs      test result: ok. 37 passed; 0 failed
Running tests\airgap_tests.rs     test result: ok. 5 passed; 0 failed
Running tests\confirm_tests.rs    test result: ok. 5 passed; 0 failed
Running tests\hd_tests.rs         test result: ok. 21 passed; 0 failed
Running tests\payment_tests.rs    test result: ok. 31 passed; 0 failed
Running tests\vault_tests.rs      test result: ok. 17 passed; 0 failed
```

Total 116 passed, 0 failed. No unused-import warnings after the final edit. Two payment tests failed on the first run: their fixture used a fake collector (`fc..fc`), which the bound path correctly refuses. The fixture now uses the real `FEE_COLLECTOR`, and the refusal assertion uses the fake address explicitly.

Test output, `cargo test -p custom-l1-node --lib core::describe` (`CARGO_BUILD_JOBS=1`):

```
test core::describe::tests::a_fee_to_the_collector_is_named_as_the_fee_not_a_recipient ... ok
test core::describe::tests::a_fee_burned_to_the_sink_is_named_as_the_fee ... ok
test core::describe::tests::a_transfer_to_the_burn_sink_is_a_send_not_a_fee ... ok
test core::describe::tests::a_second_output_to_anyone_else_stays_a_recipient ... ok
test core::describe::tests::describing_a_transfer_twice_gives_the_same_words ... ok
test core::describe::tests::every_description_is_a_sentence ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Decimals (ADR-068) untouched. No consensus code changed (describe and wallet only).

## Gaps (not done or not verified)

- Miner pinning needs CI wiring. `MAYA2C_MINER_SHA256SUMS` is not set by `release-binaries.yml`, so release Hubs refuse every miner until it is. The release `SHA256SUMS` lists archives and installers, not the raw miner executable, so CI must emit a raw miner line.
- Time-of-check to time-of-use: the hash is checked, then the path is spawned. A writer inside the install folder could swap the file in between. Not mitigated (copy to a private location before spawn is the fix).
- Key-file ownership and ACL check (P5 second test, `hub_refuses_key_file_not_owned_by_user`) not implemented; only the location check is.
- The settings UI still shows the miner-program field; the backend ignores its directory.
- `describe` runtime coverage is still 6 sample kinds. The 56-arm `match` has no wildcard, so an omitted kind fails to compile, but no runtime test calls all 56 (payload fixtures needed).
- Not built: Tauri bundle, wallet-gui `src-tauri` (separate workspace; its own `sign_transfer` command calls `Wallet::sign`, which is unchanged), the explorer.
- Examples are compiled by `cargo test` (default) but not run against a node.
- Branch not rebased onto `master`; it sits on `feat/clear-sign-screens` as specified.
