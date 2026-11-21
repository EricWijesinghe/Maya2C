# ADR-151: Node-local peer and RPC bounds from the 2026-11-19 AI audit

- Status: Accepted (re-freeze, Eric's 2026-11-17 pre-approval of node-local hardening)
- Date: 2026-11-22
- Related: ADR-147 (node-local network bounds, #589), ADR-016 (launch scope)

## Context

An AI review of the P2P and RPC layers (2026-11-19, not an external audit) listed
resource and exposure findings in `crates/node/src/network/`, `crates/node/src/rpc/`
and `bins/maya2c-node/`. Each was checked against the code before a change was made.
None of them touches a validity rule, a state root, a gas figure or a consensus
message: they change what one node keeps, refuses, forwards or reports. Two honest
nodes with different settings here still agree on every block.

The pieces that sit on the same lines as #589 (block-sync request budgets, the RPC
rate table, the market HTTP server's connection handling) are applied on #589's
branch under ADR-147. This ADR covers the rest.

## Decision

Peer-to-peer:

1. **Gossiped blocks are relayed once per block id.** A gossiped block whose header
   id this node already accepted from gossip is ignored (not relayed, not scored).
   The id is remembered only after acceptance, so a mismatched body under an honest
   header cannot shut the honest body out. 4,096 ids, oldest evicted. Gossipsub's
   message id stays author plus sequence number: the DAG-BFT engine resends identical
   frames to recover losses, and a content id would refuse those resends.
2. **`/l1/bft` is scored** with the same topic parameters as blocks and transactions,
   so malformed frames reach the graylist.
3. **Inbound connections are capped per source.** 192 of the 256 established slots
   may be inbound. One source (an IPv4 address, or an IPv6 /48) holds at most 8
   established inbound connections and 4 handshakes in flight. Loopback and
   memory-transport peers are not counted (a tunnel or a one-machine network sees
   every peer on 127.0.0.1). Bootnodes pinned with `/p2p/<id>` are exempt from the
   caps.
4. **Bootnodes must pin a peer id** at a public or DNS address, unless the operator
   passes `--allow-unpinned-bootnodes`. Local addresses (loopback, private ranges)
   are accepted unpinned for devnets. The k8s manifest and the operator, whose pod
   keys are generated at first start, pass the flag. The redial loop now keeps
   redialling while any pinned bootnode is missing, not until any peers are connected.
5. **The quarantine table holds its cap.** When every tracked peer is quarantined,
   the quarantine closest to expiry is released early to make room, and the guard
   lifts its swarm block with the normal release. Trade-off: a source with many
   identities can shorten the quarantine of the oldest offender; before, it could grow
   the table without bound.
6. **Address filtering** also refuses CGNAT (100.64/10), 0/8, the IETF and
   documentation ranges, 198.18/15, 240/4, IPv6 documentation and Teredo, and judges
   NAT64 and 6to4 addresses by their embedded IPv4 address.
7. **Post-quantum session rotation is paced.** A stale session is rotated only once it
   is at least 10 sweeps old (5 min in production), oldest first, at most 4 per sweep,
   so a syncing node that crosses several epochs does not drop every link at once.

Catch-up:

8. **The attested catch-up path has a 1 GiB byte cap** on the blocks it holds before
   checking the hash links, as the trusted path already had.
9. **Resuming as a builder needs every answering `--catch-up-from` source to agree.**
   One answering "no" keeps the node following attested blocks. An unreachable source
   abstains; no answers at all means "no". A lying source can only hold a node back
   from building, never make it build while far behind.

RPC:

10. **Supply is computed once per tip** (`market::SupplyCache`, single-flight) for
    `get_supply` and the market endpoints. The account walk runs outside the chain
    lock and is repeated if the tip moved during it; after two moved tips it runs
    under the lock.
11. **Request bodies are capped** at two maximal transactions plus 64 KiB and must
    arrive within 30 s.
12. **Many-record reads run on blocking threads** (`get_supply`,
    `get_balance_at_height`, `get_headers`, snapshot reads), and
    `get_balance_at_height` charges a per-client read budget (100,000 reads/s,
    burst 131,072) of `2 × depth + 3` before it walks. `get_headers` reads outside
    the chain lock; the caller links the headers itself.
13. **Storage and task failures answer `-32603 "internal node error"`**; the detail goes
    to the operator's log. Validation refusals keep their text.
14. **`get_peers` drops direction, score and quarantine state** unless the server is
    bound to loopback with peer addresses enabled (`with_peers`).
15. **`threat_indicators` is paged:** optional `[after, limit]`, at most 256 per page.

## Not done here

- Explicit gossipsub peers on every transport (audit P2P-01): changing it changes how
  validators reach each other, so it is a separate draft for the owner.
- Signature checks for DAG-BFT frames at the gossip layer, the source peer carried to
  the engine, IP-level quarantine, DHT address filtering, and incremental supply
  counters in state (a state-format change).
- The read budget and catch-up cap are set, not measured against a disk.

## Consequences

- Operators with an unpinned public or DNS bootnode must pin it or pass
  `--allow-unpinned-bootnodes`; the node refuses to start otherwise and says why.
- Clients that parsed internal error text, or `get_peers`' health fields from a public
  node, now get the fixed message and no such fields.
- Wire format, block validity and state are unchanged; no conformance vectors change.
