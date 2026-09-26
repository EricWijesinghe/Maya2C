# 22 — Accounts without pain

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 22

> DONE WHEN: smart accounts, policies, sponsored fees and guardian recovery
> pass tests; the phishing test set is 100% flagged; the usability study
> results are recorded against the beat bar; prior-art file updated.

| Condition | Result |
|---|---|
| Smart accounts, policies, sponsored fees, guardian recovery | **pass**: `crates/smart-account`, 10 tests with real ML-DSA-65 |
| Phishing test set 100 % flagged | **yes**: 220 of 220, each with its family's warning (`crates/clear-sign`) |
| Usability study (20 people new to crypto) | **not done**: it needs participants; there is no reference wallet UI to test |
| Prior-art file updated | `docs/prior-art/native-smart-accounts.md`, `clear-signing.md` |

## Smart accounts

```
$ cargo test -p maya-smart-account --profile ci
test result: ok. 10 passed; 0 failed …
```

The tests cover:

- a key hash on the wire (3,465 B against 5,417 B);
- rotation that keeps the address and rejects the old key;
- m-of-n thresholds;
- validation cost bounded before any signature is verified;
- daily and per-recipient spending limits;
- scoped, expiring session keys;
- vault delays with guardian cancel;
- guardian recovery with an owner cancel window;
- fee quotes as a maximum, with sponsor caps;
- replay refusal.

**Not the default.** The node's transaction format still uses hybrid-key
accounts; smart accounts are a library (ADR-016 names their validation launch
core). "Every account is programmable from genesis" is **not** true today.

## Clear signing

`crates/clear-sign` has three parts:

- **the intent standard (`Effect`)**: transfer, approve, approve-all,
  permit, upgrade, owner change, call;
- **plain-language rendering**: dangerous words in capitals, the same text
  for phone, explorer and hardware wallet;
- **a pre-sign review.** It compares the app's claimed effects with the
  wallet's simulation and checks the *simulated* effects against drainer
  patterns. It never trusts the claim.

```
$ cargo test -p maya-clear-sign --profile ci -- --nocapture
220 malicious cases in 11 families: {"address poisoning": 20, "approve-all": 20, "balance drain": 20, "claim mismatch": 20, "fake airdrop": 20, "hidden approval": 20, "malicious upgrade (Bybit shape)": 20, "owner/key change": 20, "permit, limited": 20, "permit, unlimited": 20, "unlimited approval": 20}; missed 0
100 ordinary transactions: 0 false alarms
test result: ok. 3 passed; 0 failed …
```

**How much this proves.** Less than the numbers suggest:

- **The set is synthesized.** It holds 11 documented families × 20
  parameter variations, not 200 captured mainnet drainer transactions. The
  brief asks for real-world patterns; these are their shapes.
- **The zero false alarms is by construction.** I wrote the benign set next
  to the rules. An independent benign corpus (real wallet histories) is
  what would measure it.
- **The review is only as good as the simulation feeding it.** No wallet in
  this tree simulates contract calls yet. The Ledger app renders transfer
  fields, not these intents.

## Not done

- A passkey-unlocked device key.

  The honest design: WebAuthn signs with P-256, which is not post-quantum.
  So the passkey only unlocks the local ML-DSA key and never authorizes
  anything on-chain.
- Paying fees in approved tokens at an oracle price.
- Hardware-wallet rendering of intents.
- Opt-in vault accounts in the node. They exist in the library.
- The usability study.
