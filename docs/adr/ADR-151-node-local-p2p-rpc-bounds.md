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
    the node had left (review of #655). Amended 2026-11-24: a caller that misses
    while a walk runs is refused (`SUPPLY_BUSY`, HTTP 503 on the market endpoints)
    instead of waiting on it, since each waiter held a blocking-pool thread for the
    length of the walk.
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

Amended 2026-11-24 (branch `sec/rpc-1124`, which carries #655 merged onto master):

16. **Connections time out.** Each RPC connection is served by hyper directly with a
    timer: 10 s to send (or wait for) the head of each request, the first included,
    so an idle keep-alive socket is closed. jsonrpsee's `serve_with_graceful_shutdown`
    sets no timer, so after one request a socket held its slot for good. A connection
    is served for at most 300 s, then finishes its request in flight within 60 s.
17. **One socket peer holds at most 32 connections** (an IPv6 peer by its /64), and
    one network at most 128 (an IPv6 /48, an IPv4 /24), of the global 1,024.
    Loopback and trusted proxies skip the shares, since every client behind them
    shares their address. When the table is full, the newcomer is served and the
    oldest connection of the network holding the most (the newcomer's own on a tie)
    is told to close, with 5 s to finish its request; shed sockets still closing may
    take the count at most an eighth over the cap, past which newcomers are dropped.
    Refusing the newcomer let 16 hosts that re-sent a small request every few seconds
    and reconnected at once keep every slot (amended 2026-11-24, review of #682).
18. **Block reads leave the chain lock.** `get_block_by_height`,
    `get_blocks_by_height_range` and `get_balance_changes` run on blocking threads and
    hold the chain lock only to copy the state handle. An archive fetch of a pruned
    body runs one at a time across all clients (a second caller is refused, not
    queued), costs half a read burst, and a range answer makes at most one. The slot
    is shared in turns (`admission::ColdTurns`): a client served more recently than
    one refused within the last 30 s, and not served since, yields to it, so one
    client asking again the moment its fetch ends cannot hold the slot; a client
    alone is never held back. After the one fetch,
    a pruned height ends the answer short. A range answer also stops once it carries a
    quarter of the response cap in hex-encoded blocks, always serving the first.
19. **The market endpoints** (merged from #589): 256 connections, 10 s head timeout,
    60 s lifetime, 5 requests/s (burst 20) per client, handlers on blocking threads,
    and a fixed error text. A loopback peer's `cf-connecting-ip` / `x-forwarded-for`
    names the client, so tunnelled clients get a bucket each; a non-loopback proxy
    (a k8s ingress) still puts its clients in one bucket.
20. **The RPC limiter table has a hard cap** (merged from #589): when a sweep frees
    nothing, the least recently used 1/16 is evicted, and a refund reaches an IPv6
    client's /64 bucket.

Amended 2026-11-24 (review of #682):

21. **Blocking handlers are bounded.** Every RPC method that runs on a blocking thread
    first takes one of 32 slots across all clients (`admission::BLOCKING_SLOTS`), or
    is refused at once with `RPC_BUSY` before it takes a pool thread. They share
    tokio's blocking pool (512 threads) with block relay, BFT catch-up, the
    engine-window and rejoin probes, pruning and DAG prepare, and a handler is not
    cancelled when its client hangs up. Snapshot loads are single-flight: concurrent
    misses for one height load it once.
22. **"Retry later" is retried.** The node's bootstrap client (`RpcBootstrapSource`)
    backs off and retries `COLD_FETCH_BUSY`, `READ_BUDGET_EXHAUSTED` and `RPC_BUSY` as
    it does the certificate budget refusals. Before, a lagging node fetching pruned
    heights failed outright whenever another client held the archive slot, and in
    `within_engine_window` that failure answered "no" and sent the node down the
    follower path.

## Not done here

- Explicit gossipsub peers on every transport (audit P2P-01): changing it changes how
  validators reach each other, so it is a separate draft for the owner.
- Signature checks for DAG-BFT frames at the gossip layer, the source peer carried to
  the engine, IP-level quarantine, DHT address filtering, and incremental supply
  counters in state (a state-format change).
- The read budget and catch-up cap are set, not measured against a disk. So are the
  archive-fetch cost, the connection timeouts, the per-peer and per-network
  connection caps, the blocking-slot count and the 30 s archive-turn memory.
- No cache of the last decoded archive: one may be hundreds of MiB, and keeping it
  resident trades the slot for memory. A client with many addresses still gets a
  turn per address; turns are fair per /64, not per operator.
- Total buffered request bodies can still reach 1,024 connections × the body cap;
  a global in-flight byte budget is a follow-up.

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
