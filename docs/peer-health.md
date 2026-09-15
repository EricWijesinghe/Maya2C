# Peer guard: Byzantine peers, quarantine, and block sync

**Status: SHIPPED, on by default.** Node-local; changes no consensus rule.

Code: `src/network/peer_health.rs` (offences, verdicts, scores, quarantine),
`src/network/gossip_score.rs` (gossipsub scoring), `src/network/sync.rs` (block
fetch), `src/network/node/guard.rs` (the driver side), `src/network/behaviour.rs`
(wiring), `src/bin/node/import.rs` (offences found on import, parent fetch).
Tests: `tests/byzantine_guard_tests.rs`, unit tests in each module,
`fuzz/fuzz_targets/block_sync_response.rs`.

## The brief, and what it maps to on this chain

| Asked | Built | Why the difference |
|---|---|---|
| Track invalid signatures, delayed propagation, double proposals, malformed frames | Invalid signatures and malformed frames score; so do body substitutions, blocks the chain refuses, and bad sync answers. Propagation delay is **recorded, never scored**. Double proposals are **not tracked** | A distant honest peer is late; scoring lateness lets an attacker get honest peers cut off by being closer. A proof-of-work block has no proposer, so two blocks at one height are a fork, and relaying both is honest |
| Quarantine above a Byzantine score: drop its gossip, route its traffic to a sandbox | Gossipsub blacklist (every message it relays or authored is dropped) + removed as an explicit peer + block list (connections closed, new ones refused). Expires, doubling on repeat, capped at 24 h. **No sandbox channel** | A sandbox is a route the attacker controls carrying traffic nobody processes. Dropped messages are counted per peer instead |
| Honest nodes build consensus proofs to repair state damaged by Byzantine peers | **Block sync**: a node fetches a missing parent from the peer that relayed its child; the block goes through `Chain::insert_block` like any other | A peer cannot damage an honest node's state — every block is validated and its state root checked (invariants 24, 26). A proof signed by "honest nodes" needs a known committee; on an open proof-of-work network a vote is free to forge. Work is the proof |
| 40% malicious takeover: full isolation, zero state corruption | 10 nodes, 4 hostile: every honest node quarantines all 4, drops their connections, keeps the honest mesh, never pools hostile traffic, never moves state | 40% of *identities* is not 40% of anything that decides the chain — see below |

## What "40% malicious" means here

Proof of work has no validator set. Identities cost nothing, and fork choice sums
work, never peers (`attack_simulation_tests.rs`,
`a_sybil_flood_cannot_forge_a_heavier_chain`). A hostile majority of peers
cannot make a node accept a lighter chain. What it can do is:

- **waste** bandwidth and CPU with invalid gossip — now rejected unforwarded and
  scored;
- **eclipse** a node — surround it so it hears no honest peer. The guard makes
  an eclipse harder (hostile peers that misbehave are cut off, connections are
  capped at 2 per peer and 256 in total) but cannot defeat a quiet one: a node
  with no honest peer at all stays on its last tip. Only more honest
  connections fix that.

## Gossip validation

`validate_messages()` is on. Nothing is forwarded until the driver reports a
verdict, which it does on every path:

| Verdict | When | Effect |
|---|---|---|
| `Accept` | a transaction the mempool admitted; a block that decodes and matches its `tx_root` | forwarded |
| `Ignore` | a duplicate; a transaction refused for state reasons (stale nonce, balance, full pool, halted module); anything from a peer already quarantined | not forwarded, not scored |
| `Reject` | undecodable bytes; a failed signature; a body that disagrees with its header | not forwarded; gossipsub P4 penalty; peer-guard offence |

Before this, a node relayed every invalid block and transaction before refusing
it, and nothing tied the refusal to the peer that sent it.

Blocks are accepted on stateless checks only, so relaying stays cheap. Proof of
work, execution and the state root are checked on import; a refusal there for a
reason inside the block becomes an `InvalidBlock` offence against the block's
gossip **author** — the peer that signed the message — reported back through
`NodeHandle::report_offence`. Never against the relay: an honest relay forwards
a block that passes the stateless checks, so blaming relays would let anyone get
honest peers quarantined with invalid blocks that cost no work to make. A
blacklisted author's messages are dropped however many hops away it is. Blocks
fetched by sync are blamed on the responder, which serves only blocks its own
chain accepted. Import refusals are
scored from an explicit list (a wrong `tx_root` or state root, a failed
signature, a broken transfer rule, a malformed encoding); anything else,
including a `NodeError` variant added later, is not scored until somebody
classifies it. That leaves one gap on purpose: the chain reports a wrong
difficulty or insufficient work with the same variant as an unknown parent, so a
block without real work is refused but not scored.

## Two scores

- **Gossipsub's**, for mesh management. Only P4 (invalid deliveries) carries
  weight; time in mesh, delivery deficits, IP colocation and slowness are off,
  because each punishes a quiet or distant honest peer. Two rejected messages
  reach the graylist.
- **The peer guard's**, which decides quarantine. Weights 25 (malformed frame)
  or 50 (everything else), a 10-minute half-life, quarantine at 100 — two
  invalid signatures, or four malformed frames, close together. It is what the
  node acts on, because explicit peers (every connection on the memory
  transport) sit outside gossipsub's graylist logic.

## Block sync bounds

`/maya/blocks/1.0.0`, CBOR. At most 16 ids per request; responses stop adding
blocks at 24 MiB and are refused on the wire past 32 MiB; 32 requests per peer
per 10 s, beyond which the answer is empty; 5 s timeout. A response may omit
blocks but not add unrequested ones, repeat one, or carry undecodable bytes —
any of which is an offence. The importer walks back at most 64 missing parents
from one relay, on its own task: one walk per relay, four at a time, each
fetched block streamed back into the import loop and its count-capped orphan
buffer. Inline, a relay answering every request just under the timeout with
another unknown parent could have stalled all imports for minutes without ever
committing an offence.

## Tested

- `every_honest_node_quarantines_a_coordinated_forty_percent_and_stays_whole` —
  four hostile nodes send forged-signature transactions and substituted-body
  blocks; each of six honest nodes quarantines all four, is disconnected from
  them and still connected to the other five; a *valid* transaction from a
  quarantined node reaches no honest node; an honest transaction reaches all;
  honest mempools hold exactly it; no honest state root moves.
- `slow_honest_peers_carrying_valid_traffic_are_never_quarantined` — a 300 ms
  peer, blocks with ancient timestamps, stale-nonce transactions: no quarantine,
  score zero, latency recorded.
- `a_node_fetches_a_block_it_lacks_from_a_peer_that_holds_it` — fetch, unknown
  id, oversized request, rate limit.
- `attack_simulation_tests.rs::peer_scoring_quarantine_and_connection_limits_are_wired`
  replaced the characterisation test that recorded none of this existed.

## Not done

- **Fork-aware sync.** Sync fetches parents by id from one relay. A node far
  behind still relies on `--sync-from` (RPC backfill); a sync that asks several
  peers for their best tip and fetches the heaviest header chain is the next
  step.
- **Persisted quarantines.** A restart forgets them.
- **Metrics.** Offence counts and quarantines are in `PeerReport` and events,
  not yet exported to Prometheus.
