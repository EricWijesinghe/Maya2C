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

1. **Gossiped blocks are relayed once per exact message.** A gossiped block whose
   bytes (BLAKE3 of the message data) this node already accepted from gossip is
   ignored (not relayed, not scored). The digest is remembered only after
   acceptance. It is keyed on the bytes, not the header id: the block id does not
   commit to multisig approvals (`Transaction::txid` leaves them out on purpose), so
   a copy with other approvals shares the honest block's id and, keyed on the id,
   whichever copy arrived first would shut the other out on every node. 4,096
   digests, oldest evicted. Amended 2026-11-22 after review of #655. Gossipsub's
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
   are accepted unpinned for devnets. The k8s manifest, the operator and the docker
   compose file, whose node keys are generated at first start, pass the flag; the VM
   and Windows join installers (`install.sh`, `join-validator.ps1`) refuse a
   bootnode without `/p2p/<id>`. A unit test (`shipped_deploy_paths_pass_the_pin_policy`)
   reads every shipped deploy path and fails on an unpinned public or DNS bootnode
   without the flag (amended 2026-11-22, review of #655). The redial loop now keeps
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

8. **Attested catch-up is crossed in byte-budgeted steps (1 GiB per round).** The gap's
   headers are linked to the attested checkpoint first (32 bytes a height held), then
   bodies are fetched in order, each checked against the linked id, until the round
   holds 1 GiB; that prefix is imported and the next round starts from it. A gap over
   the budget (a full epoch of 5 MiB blocks is about 18 GiB) takes several rounds and
   is never refused. Amended 2026-11-22: the first version refused such a gap, which
   left an honest node that far behind stuck on every source until a snapshot restore.
   Under a `--trusted-block` (`Anchor::Checked`) whose height lies inside the gap, the
   trusted id is compared with the linked header id at that height before any body is
   fetched, so a round that returns a prefix short of the trusted height never returns
   blocks from a chain that holds another block there (amended 2026-11-22, review of
   #655: the first stepped version checked only the returned prefix).
9. **Resuming as a builder needs every source that can judge to agree.** A
   `--catch-up-from` source at or above this node's height judges: one whose ancestor,
   epoch or round check fails answers "no" and keeps the node following attested
   blocks. A source that is unreachable, or whose tip is below this node's, abstains:
   it cannot measure this node's window, and in a rolling restart a peer that is
   itself restarting must not send every node that lists it into follower mode (more
   than f followers is the checkpoint-quorum stall of 2026-10-05 and 2026-10-18). No
   judging answer at all means "no". Unanimity rather than a majority of judging
   sources: a lying source can then only hold a node back from building, never make
   it build while far behind, and an honest source at or above this node that says
   "no" is reporting a real gap. Amended 2026-11-22 after review of #655.

RPC:

10. **Supply is computed once per tip** (`market::SupplyCache`, single-flight) for
    `get_supply` and the market endpoints. The account walk reads a `RocksDB` snapshot
    taken under the chain lock together with the tip it reports (`StateDB::view`), and
    runs outside the lock: consistent with that tip however many blocks land during
    it, and never repeated or moved under the lock. Amended 2026-11-22: the first
    version walked under the lock after two moved tips, which is when the chain is
    busiest. A cached report is served only for the exact (height, tip) it was walked
    at, and each walk replaces it: fork choice is heaviest work, so a reorg can lower
    the height, and a rule that served any newer cached height kept a branch's supply
    the node had left (review of #655).
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
    A page is read by seeking to `after` in the store (at most `limit` records), on a
    blocking thread and outside the chain lock, so paging through N indicators costs
    O(N) reads in total rather than one full scan per page.

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
  Release note for the rollout: before upgrading a live node, check its
  `node-config.toml` `[network] bootnodes` and any `--bootnode` in its service unit
  (`infra/testnet-vm` writes `BOOTNODES` into `/etc/maya2c/v<i>.env`) for an address
  without `/p2p/<id>`.
- Clients that parsed internal error text, or `get_peers`' health fields from a public
  node, now get the fixed message and no such fields.
- Wire format, block validity and state are unchanged; no conformance vectors change.

## Amendment 2026-11-24 — frame attribution, source memory, DHT filter

Still node-local: no wire format, validity rule, state root or consensus message
changes, so no conformance vectors change. Four items from the 2026-11-24 p2p
security pass, each with a test that fails without it:

16. **A forged DAG-BFT frame is blamed on its gossip author** (audit P2P-N1, the
    frame half of P2P-02). Frames are still relayed once they decode, so the
    engine is where a forgery shows. `MlDsaAuthenticator` counts the signatures
    it refuses; the driver compares the count around one frame (the engine
    verifies as it handles a frame, never later) and sets `Step::forged`, and
    an attestation outside the committee, of the wrong length, or failing
    ML-DSA sets it too. The binary then reports `Offence::MalformedFrame`
    against the frame's gossip author, never the relay, as for blocks. A frame
    for another epoch is not verified and is not judged. Tests:
    `bft_forged_frame_tests`, `auth::every_refused_signature_is_counted_once_across_clones`.
17. **The frame window is split by author.** One gossip author may hold an
    eighth of the frame bytes (32 MiB) and of the slots (64), so one identity
    can no longer fill the window and get honest frames refused. Test:
    `bus::one_author_flooding_frames_cannot_crowd_out_another`.
18. **A quarantined peer's source is remembered** (audit P2P-05). Its IPv4 address
    or IPv6 /48 refuses handshakes until the quarantine ends, at most 1,024
    sources, the soonest-ending evicted first; loopback and reserved peers are
    never banned. Tests: `ip_limits::a_fresh_identity_from_a_quarantined_source_is_refused`
    and the three beside it.
19. **DHT routing entries are filtered like identify claims** (P2P-03 residual):
    on `RoutingUpdated`, non-routable addresses are removed from the table
    unless `--allow-private-peers` is set.

Not done in this amendment, and why:

- Eight fresh identities can still fill the frame window together until each
  is quarantined. Reserving a share for committee members needs a
  peer-id-to-validator map, which does not exist (the same gap as P2P-01).
- Verifying a frame before it is relayed (deferred gossip validation) would stop
  junk at the first hop; it changes when honest frames are relayed and needs a
  multi-machine latency measurement first.
- A node behind a tunnel (loopback listener) still gets no per-source caps
  (P2P-N2). The node now warns at start-up; keying the caps on the tunnel's
  client-IP header needs the WebSocket upgrade headers, which the libp2p
  transport does not expose. A rate rule at the tunnel is the operator's cap
  until then (changing the live tunnel config needs `APPROVED:`).
- Block-sync reads still run on the swarm task (P2P-04 residual); moving them
  to a blocking worker changes the request/response flow and is a separate change.
- A validator whose signer fails sends an empty signature, which now counts as a
  forged frame against it. A signer that fails is already costing it every vote.
