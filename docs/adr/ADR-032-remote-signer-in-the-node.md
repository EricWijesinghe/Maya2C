# ADR-032: The remote signer in the node — protection keyed to the DAG, signatures unchanged

**Status:** Accepted
**Date:** 2026-09-29
**Amends:** ADR-022 (remote signer backends), ADR-027 (DAG-BFT in the node)

## Context

ADR-022 built `crates/signer`, a separate process that holds the validator
key, refuses anything slashable, and talks to the node over a mutually
authenticated post-quantum channel. ADR-027 wired DAG-BFT into the node, and
it signs with a local key file. The signer has never been called. LAUNCH.md
Gate 2 lists that as the one open software item.

Reading both sides before wiring them together (2026-09-29) found that the
signer's protection model does not fit the engine:

1. **One message per round is wrong for votes.** `SlashingDb::approve` keys
   records by `(kind, round)`, allows one message per key, and refuses a
   different one as a conflict. In DAG-BFT a validator votes for *every*
   author's vertex in a round, so it signs up to *n* different vote messages
   per round. The second honest vote of a round would be refused, and
   consensus would stall.
2. **A watermark is wrong for votes.** `approve` also refuses any round at or
   below the highest round signed. DAG votes do not arrive in round order: a
   vertex from a slow author can be voted on after later rounds have been
   signed. That, too, refuses honest work.
3. **The signed bytes differ.** The signer signs
   `"maya2c consensus signing v1" ‖ kind ‖ round ‖ payload`; the node signs,
   and every validator verifies, `VOTE_DOMAIN ‖ digest`
   (`consensus/bft/auth.rs`). A signature from the signer as built would fail
   every other node's verification.
4. **The engine does not say what it is signing.**
   `Authenticator::sign(&Digest)` carries no kind, round or author, so a
   signer could not apply any protection rule even if it had the right one.

Wiring the pieces together as they are would therefore have produced a
validator that halts at its second vote. Standing Order 9 says to report a
finding like this rather than work around it, and this ADR is that report,
together with the decision.

## Decision

**The signatures on the wire do not change.** A validator signs exactly the
bytes it signs today (`VOTE_DOMAIN ‖ digest`), whether with a local key or
through the remote signer. No consensus rule, spec vector or verification
path changes, and a network can mix local-key and remote-signer validators.

**The engine tells the authenticator what it is signing.** `Authenticator::sign`
takes a `SignContext { kind, round, author }` next to the digest:

- **`Proposal`:** this validator's own vertex; `author` is itself.
- **`Vote`:** a vote for `author`'s vertex in `round`.

This is information for protection only. It is not signed, and the local-key
authenticator ignores it.

**The signer's protection follows the DAG's actual slashing rule.** The one
slashable act in DAG-BFT is signing two different digests for the same
`(round, author)` slot: equivocating as an author, or voting for two
conflicting vertices of one author in one round. Records are therefore keyed
by `(round, author)`, over the digest:

- Signing the same digest for a slot again is an idempotent retry and is
  allowed.
- Signing a different digest for a slot already signed is refused, whether
  the kind is proposal or vote. A proposal and the author's own vote are the
  same signature (`crates/dag-bft/src/auth.rs`), so they share one slot.
- There is no watermark for votes. For proposals, the watermark stays
  (never propose at or below a round already proposed), because an honest
  validator's own rounds only ever increase.
- The record is durable (fsync) before the signature is released, as before.
- **The history is bounded.** A security review of the first draft of this
  change found that records grew without limit, so an authenticated but
  misbehaving node could fill the signer's disk. The signer now keeps the
  1,000 rounds below the highest round signed and refuses every request
  below that floor. That is safe: the engine never signs more than
  `GC_DEPTH` (50) rounds behind its last commit. It also refuses rounds more
  than 1,000 past the highest, and author ids of 1,024 or more. The file is
  compacted (temporary file, fsync, rename) once it holds more than twice
  the live records.

**The signer signs the node's bytes.** Its request becomes
`{ kind, round, author, digest }`, and it signs `VOTE_DOMAIN ‖ digest`, the
same message the node verifies. The signer's own `"maya2c consensus signing
v1"` framing goes. Domain separation from every other signature in the
system is kept by `VOTE_DOMAIN` itself, which is already unique to consensus
votes.

**The node calls it synchronously, with a deadline.** The engine is sans-IO
and `sign` is synchronous. The remote authenticator holds one open channel,
sends the request and waits up to a configured deadline (default 2 s, well
under the anchor timeout). On any failure (a refusal, a timeout or a broken
channel) it returns an empty signature. The engine already treats an empty
signature as "this validator did not vote", which costs liveness at worst,
never safety. It reconnects on the next call.

## Consequences

- An operator can move a validator's key off the validator host
  (`--remote-signer <addr> --signer-pubkey <hex>`) without any other
  validator noticing.
- A signer refusal is now a safety guarantee that is independent of the
  node's own safety log. Two protections, either of which alone prevents the
  slashable act.
- One round-trip is added per signature. On a LAN it is small next to a
  1-second anchor timeout; over a WAN it must be measured before anyone
  relies on it.
- The signer's interchange format gains the `author` field. Nothing has
  used the old format outside tests, so there is no migration.
- Still not externally reviewed: neither the channel (ADR-022) nor this
  protection rule. Both are on the audit list.

## Status of this ADR's implementation

Tracked in PROGRESS.md and in `features.toml` (`remote-signer`). The tests
that pin it:

- a validator whose key lives only in a signer process takes part in
  consensus;
- the signer refuses a conflicting vote for one `(round, author)` and
  accepts every other author's vote in the same round;
- the signatures the signer produces verify under the node's unchanged
  `MlDsaAuthenticator::verify`.
