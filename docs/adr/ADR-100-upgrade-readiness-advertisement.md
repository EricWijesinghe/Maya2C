# ADR-100: Upgrade readiness is advertised, never enforced

Status: proposed (post-genesis, draft PR)
Date: 2026-11-11
Related: ADR-082 (upgrade schedule), ADR-095 (v2 umbrella)

## Context

A scheduled protocol upgrade activates at a height. Operators need to see,
before that height, how much of the committee is running a binary that
implements the new version. Today nothing on the wire says which protocol a
peer supports, so the question cannot be answered from the network.

## Decision

1. Each node advertises the newest protocol version its binary implements,
   in the libp2p identify agent version: `maya2c/proto/<N>`.
   Identify already exists and already carries a string per peer, so no new
   wire message is added. A node that predates this sends the default
   `rust-libp2p/...` string, which parses to "no claim".
2. The receiving node parses that string with a length bound (64 bytes) and
   a strict digits-only form. Anything else is no claim. The claim is stored
   per open link and shown in `get_peers` as `claimed_protocol`.
3. A node reports its own staking seat id in `get_bft_status`
   (`validator_id`, hex). This is self-reported and is used to join a
   seat to a peer.
4. `get_next_upgrade` returns the next schedule entry activating after the
   chain height, read from committed state.
5. The status page joins these: for the next upgrade's version it shows the
   share of committee stake whose seat's peer is claimed at that version or
   later, in basis points (integer arithmetic, floored). For each seat, the
   lowest claim any watched observer reports counts. A seat no watched node
   can see is unobserved, not ready.

## Consequences

- Readiness is advisory. It does not weight votes, does not enter any state
  root, and does not halt or fork anything. The halt rule (ADR-082) is
  unchanged.
- A peer can lie about its version. Every figure on the page is labelled as
  a peer claim. The measure shows how much stake claims readiness, not how
  much stake is ready.
- Coverage is limited to seats some watched node can see. Unobserved stake is
  reported separately so the share is not read as complete.
- A new `BftStatus` field makes that type non-`Copy`. Its callers clone it.

## Alternatives considered

- Add a validator id to identify at construction. Rejected: the network is
  built before the signing key is loaded, so the id is not known then.
- A new handshake or gossip message carrying readiness. Rejected: a wire
  change for a signal that is read and never acted on, and it would need a compatibility path.
- Gate the upgrade on readiness. Rejected: the activation rule is a
  consensus rule, and the claim is unverifiable.

## Verification

- Parser tests: a `maya2c/proto/N` string parses to N; foreign, malformed,
  overflowing and oversized strings parse to none.
- Link book, `get_peers` and `get_bft_status` tests cover the claim lifecycle
  and backward-compatible parsing of answers without the new fields.
- Status tests cover the integer share (empty, all, none, floor, overflow) and
  the seat join (lowest claim, unobserved seat, no pending upgrade).
- NOT VERIFIED: end-to-end on a live multi-node network. The status page has
  not been run against real nodes.
