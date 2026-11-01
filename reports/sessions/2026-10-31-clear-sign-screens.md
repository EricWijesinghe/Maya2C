# 2026-10-31 — Clear-signing: wallet confirm screen bound to signed bytes

Branch `feat/clear-sign-screens` (worktree `D:/Maya2C-wt-wui`, off master).

## Goal

Signers must never blind-sign. The wallet confirm screen must show what the
node's `describe()` produces from the transaction that will be signed, and
signing must refuse anything other than what was reviewed.

## What changed

- `apps/wallet-gui/core/src/confirm.rs` (new): `Confirmation` and
  `confirmation(tx, chain)`. Computes `describe()` from the transaction, a
  `signing_hash` (BLAKE3 of `Transaction::signing_bytes`), the chain id, the
  sender, and for transfers the recipient (checksummed hex, display-only
  case checksum), amount and fee (display units) and a fee-above-amount flag.
- `apps/wallet-gui/core/src/payment.rs`: `sign_with_fee_to` split into
  `transfer_tx` (builds the unsigned transaction with the signer's public keys,
  the bytes a signature covers) and `finish_transfer` (signs that value).
  Signing no longer rebuilds the transaction.
- `apps/wallet-gui/core/src/wallet.rs`: `Wallet::confirm` (preview, signs
  nothing). `Wallet::sign` takes `confirmed_hash` and returns
  `WalletError::Signing` before signing unless it equals the hash of the
  transaction built from the same arguments. Refusals record no history.
- `apps/wallet-gui/src-tauri/src/commands.rs`: `preview_transfer` returns the
  core `Confirmation` (takes `fee_to` and `genesis`, so the chain and collector
  are part of the reviewed bytes). `sign_transfer` takes `confirmed_hash`.
  The old `TransferPreview` summary struct is removed.
- `apps/wallet-gui/ui/src/bridge.rs`, `ui/src/screens.rs`: the frontend stores a
  `Review` (the `Confirmation` plus the exact inputs and fee terms it was built
  from) and signs by replaying those, never the live form fields. The confirm
  screen renders only core-computed fields (description, full recipient,
  amount, fee, chain id, sender, nonce, signing hash, describe details). The
  frontend's own summary and its numeric fee warning are removed.
- `apps/wallet-gui/core/tests/confirm_tests.rs` (new, 5 tests).
- `features.toml`: `wallet-core-gui-shared` gains `confirm_tests.rs` in its
  tests list and a note. Status unchanged (`verified`).

## Tests and real output

Disk before build: `df -h /d` 401G size, 244G available (over the 60 GB floor).

`CARGO_BUILD_JOBS=2 cargo test -p maya-wallet-core` (exit 0):
```
unittests src/lib.rs      36 passed; 0 failed
tests/airgap_tests.rs      5 passed; 0 failed
tests/confirm_tests.rs     5 passed; 0 failed
tests/hd_tests.rs         21 passed; 0 failed
tests/payment_tests.rs    31 passed; 0 failed
tests/vault_tests.rs      17 passed; 0 failed
```
`cargo test -p maya-wallet-core --test confirm_tests`, final source (after rustfmt), exit 0:
```
test every_kind_reaches_the_confirmation_as_describe_wrote_it ... ok
test a_transfer_confirmation_shows_recipient_amount_fee_and_chain ... ok
test signing_with_the_reviewed_hash_signs_the_reviewed_bytes ... ok
test signing_with_a_mismatched_hash_is_refused ... ok
test a_payload_changed_between_review_and_sign_is_refused ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; finished in 0.60s
```
Test coverage, what each proves:
- describe reaches the preview: six kinds (Transfer, CancelOrder, FinalizeDispute,
  CancelProposal, LockStake, Vault Cancel) produce `description == describe(tx).summary`.
  This is a sample, not all 56. Exhaustiveness lives in `describe_kind`'s match.
- Mismatched hash refused: a reviewed hash presented with another amount, and a
  bogus hash, both return `WalletError::Signing`; history stays empty.
- Tampered payload between preview and sign refused: recipient, amount, fee,
  nonce and chain each refused; the untouched inputs then sign.
- Reviewed bytes = signed bytes: the signed raw tx decodes, its BLAKE3 signing
  hash equals the reviewed hash, and `verify` passes.
- Checksum unit test (`confirm.rs`): case-only change, hex round trip.

`CARGO_BUILD_JOBS=2 cargo check -p maya-wallet-gui --tests` in
`apps/wallet-gui/src-tauri` (exit 0, `Finished dev profile in 29m 20s`; no
`warning:` or `error` lines). A placeholder `ui/dist/index.html` was needed for
`generate_context!` and was removed afterwards.

`CARGO_BUILD_JOBS=2 cargo test -p maya-wallet-gui` (exit 0):
```
test commands::tests::ipc_amounts_above_u64_arrive_exactly ... ok
test commands::tests::malformed_ipc_amounts_are_refused_naming_the_field ... ok
test result: ok. 2 passed; 0 failed
```

`cargo check --target wasm32-unknown-unknown` in `apps/wallet-gui/ui` (exit 0):
`Checking maya-wallet-ui v0.1.1`; the only warning is a future-incompat notice
from the dependency `proc-macro-error2 v2.0.1`.

Not run: the e2e crate (`apps/wallet-gui/e2e`, makes no preview or sign calls),
a browser or Tauri run of the send screen, and `cargo clippy`.

## Hub

`apps/hub` does not sign transactions. `apps/hub/src-tauri/src/miner.rs:4`:
"The hub never hashes or signs." Its only signing reference is the update
verifier in `Cargo.toml` (lines 30-31). No change.

## Gaps left (not built here)

- Ledger app: `apps/ledger-maya2c/src/review.rs:1-17` parses the signing bytes on
  the device and refuses anything that is not a v7 transfer (`:75`). It has no
  `describe()` equivalent, so non-transfer payloads are refused, not described.
  `apps/ledger-maya2c/src/main.rs:272` is where the review is shown. Not built, per
  instruction.
- `apps/wallet-gui/core/src/payment.rs` still exports `sign_transfer` and
  `sign_transfer_to` with no confirmation gate. They serve
  `apps/wallet-gui/core/examples/devnet_send.rs:47,57`. The GUI cannot reach them.
- `apps/wallet-gui/core/src/compose.rs:165` signs in `compose_swap`. No command
  calls it (`rg compose_swap apps/wallet-gui/src-tauri` is empty). It is still
  an unconfirmed signing path in the core.
- Typed recipients are not checksum-validated on input. Only the displayed form
  is checksummed. A mixed-case typo is not caught on entry, only visually.
- `describe` counts the fee output as a second recipient (`describe_transfer`,
  `crates/node/src/core/describe.rs:112-128`). A transfer with a fee reads "to 2
  recipients" and its total includes the fee. The fee output is shown separately
  in the screen's Fee line and in `details`, but the summary sentence is describe's.
- Unit mismatch to resolve: `amount::DECIMALS` is 24 (`apps/wallet-gui/core/src/amount.rs:11`),
  and the frontend's `typed_amount` comment says 18 decimal places
  (`ui/src/screens.rs`, "at most 18 decimal places").
  `format_display` also does not trim trailing zeros, so amounts show as
  `1.500000000000000000000000`. Neither was changed here.
- `STATE.md` still says the GUI shows less than `describe`. Not edited; the next
  handover should cite this report.
