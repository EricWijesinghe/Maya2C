# Threat-intel registry

**Status: RESEARCH.** `THREAT_INTEL_ACTIVATION_HEIGHT = u64::MAX`
(`crates/node/src/state/context.rs`). The code ships and is tested; no network runs it.

A node that receives a gossip message whose bytes fail a check any node can
re-run turns the author's own signature into **evidence**. A transaction
records that evidence on chain as an **indicator** against the author. Every
host's firewall worker then drops the addresses that author connected from, for
as long as the indicator's score says.

| Piece | Where |
|---|---|
| Signed bytes, score, decay, record, mitigation (dependency-free, Kani) | `crates/threat-intel/` |
| Signature capture from gossipsub | `crates/node/src/network/evidence_tap.rs` |
| Evidence emission, per-peer address book | `crates/node/src/network/node/threat.rs` |
| `TxKind::AttestAttack` wire form | `crates/node/src/core/threat_payload.rs` |
| Verification and recording | `crates/node/src/state/threat_exec.rs`, `crates/node/src/state/threat.rs` |
| RPC: `threat_indicators`, `threat_peer_addresses` | `crates/node/src/rpc/server.rs` |
| Per-host worker (nftables / iptables / dry run) | `bins/threat-firewall/` |
| End-to-end and refusal tests | `crates/node/tests/threat_intel_tests.rs` |
| Decoder fuzzing | `fuzz/fuzz_targets/threat_evidence_decode.rs` |

## What the brief asked for, and what this is instead

The brief asked for six things this chain cannot provide as stated. Each
reconciliation is listed in [trajectory.md](trajectory.md), item 11.

| Brief | Why not | Instead |
|---|---|---|
| ZK proofs of DDoS floods and port scans | Neither leaves anything a third party can verify. A UDP source is forgeable, and "it sent me a million packets" is the observer's word | Evidence covers only offences that leave checkable bytes. It needs no ZK, because it hides nothing |
| A consensus-driven threat score | Proof of work has no validator set, and identities are free, so any score counted by head belongs to whoever runs the most keys | No votes. One piece of verified evidence confirms, and no number of unverified ones does anything |
| IP quarantine recorded in state | No proof binds an address to a key, so every recorded address is a lever to censor an honest host | Indicators are keyed by author. Each host maps an author to the addresses of its own connections |
| "Within 2 DAG rounds" | Nothing in consensus has rounds | 2 **blocks**. The test enforces at the recording height itself |
| One worker updating firewalls "across enterprise infrastructure" | A central rule pusher is a lateral-movement control plane | One worker per host, reading its own node and pushing nothing |
| (Implied) the registry stops a DDoS | Keys are free | It stops an identity that sent provably invalid data from being accepted by any node. Volume stays with the XDP token bucket |

## Evidence

### Offences that can be attested

`maya_threat_intel::OffenceKind` defines two:

- **`InvalidSignature`**: a frame on `/l1/txs/1.0.0` that decodes as a
  transaction and fails `Transaction::verify` with `SignatureVerification`,
  `HashSignatureVerification` or `MissingSignature`. These are the errors
  `peer_health::classify_transaction` scores.
- **`TxRootMismatch`**: a frame on `/l1/blocks/1.0.0` that decodes as a block
  whose body fails `check_tx_root`.

The other peer-guard offences are excluded on purpose:

- **A frame that fails to decode.** Decoders gain formats, so "does not decode"
  can be true on an old node and false on a new one. A consensus rule that
  flips with the node version is a chain split.
- **An invalid block.** Whether a block is invalid depends on state, so a
  third party with different state could not re-check it.
- **A bad sync response.** It is not gossip, so it carries no author
  signature.

### The signed bytes

libp2p-gossipsub 0.50 signs `"libp2p-pubsub:" || protobuf(Message)` over these
fields:

| Field | Number | Content |
|---|---|---|
| `from` | 1 | The author's peer id: `00 24 08 01 12 20 ‖ key` |
| `data` | 2 | The frame |
| `seqno` | 3 | 8 big-endian bytes |
| `topic` | 4 | The topic string |

`SignedGossip::signed_bytes` rebuilds these bytes itself instead of calling
libp2p. A libp2p upgrade that changed the encoding would therefore make new
evidence unverifiable, but could never make old evidence verify differently.
The topic is part of the signed bytes, so evidence cannot be moved between
offence kinds. `crates/node/tests/threat_intel_tests.rs` checks the reconstruction against
signatures captured from a live mesh: every indicator in the end-to-end test was
verified over bytes this crate rebuilt.

### Capture

Gossipsub verifies the signature in its codec (Strict mode, before the behaviour
sees the message) and then drops it: the `Message` it hands the application has
no signature field. `EvidenceTap` is the node's gossipsub `DataTransform`, the
one hook that sees the `RawMessage`. It puts the signature, length-prefixed, in
front of the delivered message data, and the driver splits it back off.

The signature travels inside the message, not in a table beside it. A table
keyed by (author, sequence number) was the first design, and a security review
showed that a flood of validly signed messages could evict entries before the
driver claimed them, silently turning evidence capture off. A signature carried
in its own message cannot be evicted and holds no memory once the message is
judged. Message ids are unaffected, because gossipsub's default id is author
plus sequence number, and forwarding uses the raw message. The node **emits**
`NodeEvent::AttackEvidence`. It never submits: submitting needs a funded chain
key, and the node holds none.

### Verification (`crates/node/src/state/threat_exec.rs`)

Checked in this order, both statelessly, first at mempool admission and again at
execution:

1. **`verify_strict`** over the rebuilt bytes with the author's ed25519 key.
   Strict, because consensus needs exactly one answer. A signature that
   libp2p's looser check accepts and strict verification refuses is evidence
   nobody can attest. That lets an author evade attestation. It never lets
   anyone frame someone else.
2. **The frame decodes and fails the named check.** A frame that passes is a
   lie about an honest author, and is refused.

### Invalid evidence is an error; repeated evidence is a no-op

An attestation is its submitter's own transaction, so evidence that fails
verification fails the block. HTLC locks work the same way.

A second copy of evidence already on chain is different. Two observers reporting
the same offence is the ordinary case. If the later copy were an error, it
would void the block the earlier one sits in, so it is a no-op instead
(invariant 7). The evidence id is BLAKE3 over the kind and the signed bytes,
**not the signature**. An author who re-signs the same message has produced the
same evidence, not more.

## Score and duration (`crates/threat-intel/src/score.rs`)

- One verified offence adds `CONFIRM_SCORE = 100`. That is the threshold, so a
  single conviction confirms.
- The score halves every `HALF_LIFE_BLOCKS = 720` blocks. Durations are measured
  in heights, never timestamps (invariant 9).
- A quarantine holds while the decayed score is at least 100. It lasts
  `floor(log2(score / 100)) + 1` half-lives after the last offence: one offence
  gives 1 half-life, two give 2, four give 3.
- The score is capped at `MAX_SCORE = 204,799`, which is 11 half-lives. Past the
  cap, more offences change nothing.

Nothing releases an author early. The height does. The Kani proofs in
`crates/threat-intel/src/proofs.rs` state:

- the closed form agrees with running the decay, for every score and height;
- where the lift height doesn't saturate, the quarantine holds exactly below it;
- `observe` keeps every record canonical, and it decodes back to itself;
- a first offence quarantines at the height that recorded it.

## State

The keys live under `t:`, which is its own layer, `StateLayer::ThreatIntel`
(tag 13), in `RECORD_LAYERS` (invariant 25):

| Key | Value |
|---|---|
| `t:in:<author>` | `ThreatIndicator`: score, first height, last height, offences (28 bytes) |
| `t:ev:<evidence id>` | Recording height |

- **Existing roots.** An absent layer folds nothing, so no chain that has not
  activated the branch sees a different root.
- **Reorgs.** Both records go through the generic record overlay, so the undo
  journal restores them on a reorg.
- **Circuit breaker.** `Module::ThreatIntel` (tag 10) gates new attestations.
  Halting them is safe: no value moves, and a recorded indicator keeps decaying
  on its own.

The record carries no address and no observer. It says what the author signed
and when that was proved. It says nothing about who proved it or where the
author connects from.

## Enforcement in the node

Whatever applies blocks pushes the tip's convictions to the node:

1. It computes `StateDB::active_mitigations(height)` at the tip.
2. It hands the result to `NodeHandle::enforce_mitigations`.

The node binary's `threat_enforcement_loop` does this beside the rotation-height
sampler, and pushes only when the set changes. It is the same arrangement as
`EpochClock::set_height`: the driver reads no chain height itself.

Each push replaces the node's convicted set:

- **Convicted peers** are refused the way a quarantined one is. Gossipsub drops
  what they relay or authored, and the block list closes their connections and
  refuses new ones. The driver emits `NodeEvent::PeerConvicted`.
- **Acquitted peers**, whose indicators have lifted, are re-admitted unless the
  local peer guard is also holding them. The driver emits
  `NodeEvent::PeerAcquitted`.
- **The two refusals are independent.** A guard quarantine that expires does not
  unblock a convicted peer, and an acquittal does not lift a guard quarantine.
- **A node never convicts itself.**

`crates/node/tests/threat_intel_tests.rs` checks this end to end. A node that never saw the
attack joins, holding the recorded chain, and refuses every attacker's
connection while admitting an honest peer. Once acquitted, the attackers are
re-admitted.

## Enforcement at the host (`threat-firewall`)

The worker polls one co-located node:

1. `get_tip_height`;
2. `threat_indicators`, which reports every indicator by author, never by
   address;
3. `threat_peer_addresses`, which reports the address of each peer's most recent
   connection **to this node**. The node remembers up to 4,096 peers past
   disconnection, because the peer guard disconnects an offender immediately.
   This method answers only if the operator wired the node handle into RPC with
   `RpcContext::with_peers`. It reveals where peers connect from, so `rpc::serve`
   refuses to start a server wired this way on anything but a loopback address.

`policy::decide` is a pure function. It blocks every address of every author
with an active mitigation and unblocks everything else. Loopback and
unspecified addresses are never blocked.

### Backends

- **`nftables`** adds and deletes elements of `threat4` / `threat6` in an `inet`
  table the operator creates once:

  ```
  table inet maya2c {
      set threat4 { type ipv4_addr; }
      set threat6 { type ipv6_addr; }
      chain input {
          type filter hook input priority -10; policy accept;
          ip saddr @threat4 drop
          ip6 saddr @threat6 drop
      }
  }
  ```

- **`iptables`** inserts one `INPUT … -j DROP` rule per address, tagged
  `--comment maya2c-threat`. It runs `-C` first, so a restarted worker does not
  stack duplicate rules.
- **`dry-run`** changes nothing and reports what it would change.

Commands are argument vectors built from a parsed `IpAddr` and a table name
checked against `[a-z0-9_]{1,32}`. They run without a shell. The worker needs
`CAP_NET_ADMIN` and holds no chain key.

### Why not XDP

The `ebpf-net` XDP program judges only relay datagrams and passes all other
traffic, and it belongs to the node's relay: a second loader would detach it.
General ingress filtering is netfilter's job. The node already mirrors its own
peer-guard quarantines into the XDP blocklist for relay traffic.

## Limits, stated

- **The evidence signature is classical.** libp2p identities are ed25519, so a
  quantum adversary who can forge ed25519 can frame any peer id. This is the
  first thing to settle before activation.
- **Evidence markers are never pruned.** Each attestation leaves a 32-byte key
  forever. Growth needs a bound, such as expiring markers once the indicator
  they fed has decayed, before any network carries them.
- **Evidence is capped at 64 KiB.** A `tx_root` substitution in a block of more
  than a handful of hybrid-signed transactions cannot be attested. The offence
  still stands locally, in the peer guard.
- **Shared addresses.** Blocking an offender's address blocks everyone behind
  the same NAT or cloud egress.
- **No DDoS defence.** Keys are free. An attacker who rotates identities after
  each conviction pays one conviction per identity, and the flood itself is
  handled by connection caps and the token bucket, not by this registry.
  `a_flood_of_valid_transactions_produces_no_evidence` pins that valid-byte
  floods yield nothing.
- **Fuzzing, first run.** `threat_evidence_decode` ran 250,591,546 inputs in
  301 s (about 830,000 per second, peak RSS 525 MB) with no crash, on
  2026-09-15. It ran in a standalone cargo-fuzz project linking only
  `maya-threat-intel`, because the in-repo `fuzz/` package links the whole node,
  and building that under ASAN crashed the 12 GB WSL VM. CI runs the in-repo
  target.
- **Kani runs on Linux only.** `cargo kani -p maya-threat-intel` (Kani 0.67.0,
  in WSL Ubuntu-24.04) verified all 4 harnesses with 0 failures on 2026-09-15.
  The proofs cover the score arithmetic and the record codec, not the node's
  evidence verification, which calls ed25519 and the hybrid verifier.
