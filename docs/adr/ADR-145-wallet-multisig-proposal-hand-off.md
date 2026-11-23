# ADR-145: Wallet multisig proposal hand-off and the hybrid-only approver rule

**Status:** Accepted, 2026-11-17 (under Eric 2026-11-14 "I approve everything").
**Date:** 2026-11-17
**Related:** [ADR-011](ADR-011-threshold-custody.md) (the m-of-n policy),
[ADR-013](ADR-013-suite-envelope-activation.md) (wire v8 live from
genesis), [ADR-029](ADR-029-fee-market-live.md) (per-byte base fee),
[ADR-036](ADR-036-chain-bound-signatures.md) (chain-bound signing bytes),
[ADR-117](ADR-117-multisig-suite-diversity.md) (per-family approval floor;
Proposed 2026-11-12, post-genesis `postgen/v4`, wire v12, dark until
scheduled). Item P-16 in the 90-day roadmap; PR #545.

Nothing here touches the node, the wire format, consensus or state. The wallet
uses the v8 frame and the `maya-crypto-pq` policy exactly as the node verifies
them.

## Context

P-16 asks for a multisig creation and approval flow in the desktop wallet on
top of the chain's existing m-of-n accounts (ADR-011). Members sit on
different machines and reach each other by any channel a person chooses
(chat, mail, a USB stick). The wallet needs a format for "a spend waiting for
approvals", a rule for what it will approve, and a rule for which accounts it
will let a person fund.

Two facts constrain the design:

- The Ledger app's review reads only the v7 transfer domain and refuses v8
  multisig bytes as not a transaction (`0x6a90`, pinned as the
  `multisig-v8-spend` case in `apps/ledger-maya2c/tests/fixtures/suite-0x10-kinds.txt`).
- No in-wallet flow produces a non-hybrid approval: there is no Ledger v8
  review and no "add external approval" command.

So today only a hybrid `0x30` key held by this wallet is *counted* as an
approver. That does not stop other approvals arriving: a proposal read from
another member (`Proposal::from_hex`, then `merge` or `with_approval`) can
carry an approval any tool made, such as an ML-DSA-65 `0x10` one signed on a
host. Rule 6 says which of those are accepted.

## Decision

1. **The proposal is the v8 transaction itself, hex-encoded**, carrying the
   approvals collected so far. Approvals sit outside the signed bytes
   (ADR-011), so members approve in any order and in parallel, and
   `Proposal::merge` combines two copies of the same spend (same signing
   bytes). There is no second format to version, and the finished proposal is
   the broadcastable frame.
2. **A proposal read from anyone else is untrusted input.** `Proposal::from_hex`
   refuses, before a confirm screen ever sees it:
   - a frame without a multisig section (any non-v8 frame);
   - any kind but a plain transfer, because the confirm screen shows a
     transfer in full and anything else would be approved from a summary;
   - any input, no output, more than two outputs, or a second output that is
     not a fee output, so every unit that leaves the account is named on the
     screen;
   - more approvals than the threshold (such a copy could never finish but
     would keep accepting approvals);
   - a policy rule 3 refuses, so an account the wallet will not open cannot
     be approved through a proposal for it either;
   - any approval that does not verify under the policy key it names, for
     this chain.
3. **Hybrid-only approvers.** An account whose threshold is above its number
   of hybrid `0x30` members is refused, both when built here
   (`MultisigAccount::new`) and when opened from shared hex
   (`MultisigAccount::from_hex`), and when read inside a proposal
   (`Proposal::from_hex`): funds sent to it could never move. Other
   post-quantum suites may sit in a policy as spare members.
4. **The account card says who you are.** Each member is shown with its suite
   and a 16-byte BLAKE3 fingerprint of the whole public key, and members whose
   key is one of this wallet's derived accounts are marked. When none is, the
   card warns that this wallet can never approve, before anyone funds the
   address. A key prefix is not used: the first bytes of an ML-DSA key are its
   public seed, which a dishonest coordinator can grind to match.
5. **Approval is a transfer's approval.** It needs this session's confirm
   screen for the exact signing hash and the native OK dialog. The confirm
   screen is built for the account open on the screen and the signing
   account: a proposal spending from any other multisig account is refused
   (`Wallet::confirm_multisig`), and the confirm text, native dialog
   included, starts with "Multisig k of n, signing as member i (account j)". Every refusal
   the core gives after the dialog (fee cap, fee for the finished frame,
   membership, repeat approval, met quorum) is checked before the dialog too
   (`Wallet::check_multisig_approval`).
6. **The fee is priced for the finished v8 frame** (policy plus the `m`
   longest signatures among the members rule 3 lets approve; a spare member
   that cannot approve never adds a signature) and refused when under `base_fee x size` at the
   current base fee. So that the price is an upper bound on what is finished,
   every import path (`from_hex`, `with_approval`, and `merge` through it)
   refuses an approval from a member whose suite signs longer than the hybrid
   signature the price counted. An ML-DSA-65 `0x10` approval (3,309 bytes) is
   accepted; an SLH-DSA-SHAKE-256f `0x21` one (49,856 bytes) is refused, since
   it would leave about 38.7 KB of a 2-of-{0x30, 0x30, 0x21} spend unpaid and
   the node would refuse it as `FeeTooLow` after members had approved
   (`an_approval_longer_than_the_priced_signature_is_refused`). Pricing from
   the approvals actually carried was not chosen: the fee is fixed in the
   signed bytes before anyone approves, so a longer approval arriving later
   could only be refused anyway. That check cannot see base-fee rises while approvals are
   collected, so the screen pre-fills the Standard tier (twice the current
   minimum) and says a fee at the minimum may be refused at broadcast.

## Consequences

- Relaxing rule 3 needs two things: a Ledger v8 multisig review (shown and
  clear-signed; the `multisig-v8-spend` fixture verdict flips) and an "add
  external approval" command. Until then a Ledger key cannot be a required
  approver. The fee estimate's approver filter and the signature-length cap
  (rule 6) relax with it: once a longer suite can be a required approver, the
  price must count its signature.
- A proposal of any other kind (contract call, staking, vault) from a shared
  account is not supported in the wallet. Adding one means a full confirm
  screen for that kind first.
- The ADR-117 per-family approval floor is not built: ADR-117 is Proposed,
  post-genesis, wire v12, dark until scheduled (activation `u64::MAX`), so
  the wallet cannot apply the floor before consensus does.
- No state is stored: the account and proposal are text the person keeps.

## Alternatives considered

- **A separate proposal envelope (JSON or a new binary frame)** with the
  transaction inside: a second format to version and check, and a second
  place for the signed bytes and the shown bytes to drift apart. Rejected.
- **Accept any account and fail at approval time:** a person would learn they
  cannot approve only after the address is funded. Rejected (rule 3).
- **A fixed fee headroom enforced at propose time:** no fixed percentage
  covers an approval period of unknown length, and it would refuse fees that
  are fine on a quiet chain. Rejected in favour of the pre-filled tier and the
  stated warning.
