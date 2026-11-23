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

## Amendment 2026-11-24 — frames checked before relay, source memory, dial filter

Still node-local: no wire format, validity rule, state root or consensus message
changes, so no conformance vectors change. Items from the 2026-11-24 p2p
security pass and its review, each with a test that fails without it:

16. **A DAG-BFT frame is relayed only after its signatures verify** (audit
    P2P-N1, the frame half of P2P-02). Gossipsub already runs with manual
    validation, so the driver now holds its verdict on a frame that decodes and
    hands the frame, its relay and its message id to the BFT loop. The loop runs
    `BftDriver::check_frame` before it takes the chain lock and settles the
    verdict at once (`NodeHandle::settle_frame`): **authentic** (every
    signature verifies against the epoch's committee and chain tag; a
    certificate must also be well formed, since an empty vote list verifies
    vacuously) is relayed; **forged** or **unjudged** (an epoch this node has no
    committee for) is dropped. The check reads only the frame and the committee,
    never the engine's state, so an observer, a validator the frame is not
    addressed to, and one that already holds the certificate all relay on the
    same terms. A forgery now stops at the first honest node instead of reaching
    every node. Tests: `bft_forged_frame_tests` (7).
    - Blame: a forged frame is scored (`MalformedFrame`) only against a relay
      that also signed the gossip message. Never against the author alone:
      strict gossip signing makes the author any keypair, so a throwaway key per
      frame escaped quarantine, and a frame an honest validator really signed
      (for example on a testnet, with the node key reused) can be replayed by
      anyone, so blaming the author let one peer get an honest validator
      blacklisted by every node (review of #684). Never against a relay
      forwarding another author's frame: a node on the previous version relays
      frames on decoding alone. Test: `guard::a_forged_frame_is_blamed_only_on_a_relay_that_signed_it`.
    - Cross-network replay needs no topic namespacing: every connection is
      bound to one chain by the Noise prologue (`network_prologue`), so another
      network's frame arrives only through a relay on this one, and fails the
      chain tag in the check. Test: `a_proposal_signed_for_another_chain_is_forged`.
    - A fetch carries no signature; one from a member's id is relayed as before.
    - Cost: a certificate is verified twice by a node that inserts it (the check,
      then the engine), and a node that already holds it now hashes and verifies
      it once before relaying. Relaying waits for the BFT loop, so a lagging loop
      delays what it relays. NOT MEASURED across machines; see below.
17. **The frame window is split by author and by relay.** One gossip author may
    hold an eighth of the frame bytes (32 MiB) and of the slots (64), and one
    connected relay half (128 MiB, 256 slots), so a link that signs each frame
    with a fresh key still takes at most half. Refusals are counted by reason
    (budget, author share, relay share) in the rate-limited warning, so a
    measurement can tell an attack from a slow reader. Tests:
    `bus::one_author_flooding_frames_cannot_crowd_out_another`,
    `bus::one_relay_signing_each_frame_with_a_fresh_key_cannot_fill_the_window`,
    `bus::a_frame_nobody_will_check_is_refused`.
18. **A quarantined peer's source is remembered** (audit P2P-05). Its IPv4 address
    or IPv6 /48 refuses handshakes until the quarantine ends, at most 1,024
    sources, the soonest-ending evicted first; loopback and reserved peers are
    never banned. Tests: `ip_limits::a_fresh_identity_from_a_quarantined_source_is_refused`
    and the three beside it.
19. **Dials are filtered in the transport** (P2P-03 residual). Kademlia dials the
    addresses in other peers' `FIND_NODE` answers while a query runs, before any
    routing event, so filtering the table alone let a DHT peer make nodes probe
    a private network. `network::dial_policy` wraps the TCP/WebSocket transport:
    a dial to a private, loopback or link-local address needs
    `--allow-private-peers`, and a DNS name is dialled only if the operator gave
    that exact address (`NodeHandle::dial`, the bootnodes). `RoutingUpdated`
    removes from the table what the policy would not dial. Tests:
    `dial_policy::learned_private_and_dns_addresses_are_never_dialled` and the two
    beside it.
20. **A re-broadcast proposal always carries the validator's signature.** The
    engine's resend took the signature with `unwrap_or_default`; the pending
    proposal always holds it (both places that set it insert it), and a missing
    one now sends nothing instead of a frame every peer refuses.

Not done in this amendment, and why:

- Relay latency of the held verdict across a multi-hop mesh is not measured: the
  four-validator localnet is a full mesh on one machine. A multi-machine
  attacknet run should count refusals by reason and compare finality latency
  before and after.
- Fetch replies (up to `4n` certificates per member per resend interval, each up
  to a full batch) could pass a 32 MiB author share while the BFT loop lags, for
  example verifying a quorum under the chain lock during catch-up. Plausible, not
  measured; the warning now names the reason, which is what a measurement needs.
  If confirmed, fetch replies move to request/response.
- A node behind a tunnel (loopback listener) still gets no per-source caps
  (P2P-N2). The node now warns at start-up; keying the caps on the tunnel's
  client-IP header needs the WebSocket upgrade headers, which the libp2p
  transport does not expose. A rate rule at the tunnel is the operator's cap
  until then (changing the live tunnel config needs `APPROVED:`).
- Block-sync reads still run on the swarm task (P2P-04 residual); moving them
  to a blocking worker changes the request/response flow and is a separate change.
