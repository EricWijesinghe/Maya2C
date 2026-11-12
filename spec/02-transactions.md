# 2. Transactions — v0.1.0

Reference: `crates/spec-ref/src/{wire,stf}.rs`. Node: `Transaction::verify`,
`Transaction::sender`, `StateDB::stage_transaction`.

- **TX-1** A transaction authorizes nothing unless **both** its ML-DSA-65 (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205) signatures verify over TX-3's bytes. This is checked before any balance rule, and after the nonce check (STF-1), which is a cheaper refusal of a stale transaction; both orders accept the same transactions. A block containing a transaction that fails it is invalid.
- **TX-2** The sender is derived, never read from the wire: `blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)`. *(positive only: defines a value)*
- **TX-3** Both signatures sign `"custom-l1-node.tx.v4" ‖ chain_tag ‖ io_section ‖ public_key ‖ nonce_u64` (plus the payload encoding for version 6), where `chain_tag` is TX-5's. The keys are inside the signed bytes, so each signature commits to the other scheme's key. Suite-tagged (v7) and multisig (v8) transactions sign under `"custom-l1-node.tx.suite.v2"` and `"custom-l1-node.tx.multisig.v2"`, each followed by `chain_tag` and then their own fields. *(positive only: defines a value)*
- **TX-5** `chain_tag` is the 32-byte id (CON-2) of the genesis block of the one chain a signature is valid on. It is signed, never sent: the wire format (ENC-2) carries no tag, and a node verifies with its own genesis id, so a frame signed for any other chain — another network, or an earlier genesis of the same one — fails TX-1. A node that has not bound its genesis verifies nothing. (ADR-036.)
- **TX-4** Suite-tagged (v7) and multisig (v8) transactions verify only through the height-aware `verify_at`, at or past `SUITE_ENVELOPE_ACTIVATION_HEIGHT` — `0` since ADR-013, so from genesis — under the suite registry's policy; `Transaction::verify`, which has no height, refuses them.

## Account authority (ADR-086) — dark

Inactive until an ADR-082 schedule entry sets `FeatureSet::ACCOUNT_AUTHORITY`
(bit 9); `ACCOUNT_AUTHORITY_ACTIVATION_HEIGHT` is `u64::MAX`. Node:
`state::account_authority`, `core::account_tx`, `core::account_payload`.

- **AA-1** Rule A/B. If `ka:a:<X>` exists, a transaction acts for `X` only as a wire-version-9 frame naming `X` whose policy digest equals the record's effective digest; every v5–v8 frame whose sender (TX-2) is `X` is refused, and so is a v9 frame naming an account with no record. With no record and no v9 frame, staging is unchanged. Checked after the nonce (STF-1) and before signatures and the fee; the mempool applies the same check at the tip. The one exception to TX-2: a v9 frame's sender is its explicit account, admitted only by this rule. *(external vectors: spec/account-authority-vectors.json)*
- **AA-2** A v9 frame is `9 ‖ account[32] ‖ <the v8 frame after its version byte>`. Every approver signs `"maya2c/account-bound/v1" ‖ chain_tag ‖ account ‖ policy_digest ‖ io_section ‖ policy ‖ nonce_u64 ‖ payload`, so an approval binds the chain (TX-5), the account and the policy and never verifies as a v8 approval. The txid hashes kind byte 9 and the same body. `Transaction::verify` refuses v9 (TX-4). *(external vectors: spec/account-authority-vectors.json)*
- **AA-3** `TxKind::Account` is payload tag 54: `0 policy` adopt, `1 delay:u64 threshold:u8 n:u8 guardian[32]*n` set guardians (n ≤ 7), `2 account[32] policy` propose, `3 account[32]` cancel, `4 account[32]` finish, `5 account[32]` withdraw. No action carries an amount or writes a balance; value outputs beside it are refused (mixed kind). *(external vectors: spec/account-authority-vectors.json)*
- **AA-4** Below activation every v9 frame and every tag-54 payload is refused; no `ka:` record can be written. *(external vectors: spec/account-authority-vectors.json)*
- **AA-5** The `ka:` prefix is state layer 18 (`maya account-authority state root v1`), folded after the vault layer and only when non-empty: a chain where no account opted in has the same state root as one where the feature is dark. *(external vectors: spec/account-authority-vectors.json)*
- **AA-6** Guardian sets hold 1–7 distinct addresses, not the account, with a strict-majority threshold and `3,600 ≤ delay_blocks ≤ 1,000,000`. The first `SetGuardians` (or `AdoptPolicy`) opts in and is immediate; later ones, and an `AdoptPolicy` once guardians exist, are queued for the current delay. Each guardian holds at most one approval, which follows its latest proposal; approvals count only from current guardians; `FinishRecovery` succeeds at `quorum_at + delay_blocks` when the quorum still holds and every key of the new policy may sign then, writing the new digest and clearing the request and queued changes. `CancelRecovery` (the account's own authority) clears the request and every queued change; it and `WithdrawApproval` are never halted by the guard. *(external vectors: crates/node/tests/account_authority_tests.rs)*

## Gaps

- TX-4's vectors (`consensus.json`) state the path, not the frame bytes: the reference does not re-implement signatures, so the node builds real v7 (ML-DSA-87) and v8 (2-of-3) frames and checks which call accepts them.
- Typed payloads (channels, DEX, governance) are specified only by the node's code today.
