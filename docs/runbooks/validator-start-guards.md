# Validator start guards

Two start-up checks that protect a validator from its own machine and its own
network. Neither changes consensus: they decide whether a node starts, and
what it dials.

## Windows power plan

A validator that sleeps misses its turns. On Windows the node reads the AC
standby and hibernate timers of the active power plan before it starts in
validator mode (`--validator-key` or `--remote-signer`).

- If either timer is non-zero, the node refuses to start and prints the fix.
- If a timer cannot be read, the node refuses. An unknown plan is not
  assumed safe.
- `--allow-sleep` starts the node anyway, with a warning. Use it only when
  you accept the PC sleeping.
- When the plan never sleeps, the node asks Windows to keep the system awake
  (`ES_SYSTEM_REQUIRED`) for as long as it runs. It does not keep the display on.
- Linux and macOS: no check, no change.

The fix, from an elevated shell (values are seconds; `0` means never):

```powershell
powercfg /change standby-timeout-ac 0
powercfg /change hibernate-timeout-ac 0
```

Check the active plan with `powercfg /getactivescheme`. Changing the timers
changes the active plan's values, so the plan you read is the plan that counts.

What the guard does not cover:

- The lid-close, power-button and sleep-button actions are separate settings.
  The guard does not read them. On a laptop, set the lid action to
  "Do nothing" on AC, or the PC can still sleep when the lid closes.
- Only AC values are read. A validator on battery is not protected by this
  check.
- The check runs once at start. A later change to the plan is not seen until
  the node restarts.

Start scripts in this repository that launch a Windows validator run the same
check first (`infra/hybrid-testnet/start-pc-validators.ps1`). It is a
preflight: the node is the authority.

## Bootnodes

How a node finds peers, in order:

1. `--bootnode <multiaddr>` flags, repeatable.
2. `[network] bootnodes = [...]` in the `--config` file. Before this release
   the file's list was read by nothing; a node configured only by file dialed
   no one.

Every bootnode must end in `/p2p/<peer id>`, so the node dials the peer it
expects and no other. A bootnode address without it is refused at start.

At start the node dials every configured bootnode. While it has fewer peers
than bootnodes, it dials them again every 10 seconds, because a dial made
before the other node was listening is never retried by libp2p.

If the node has no peer at all for 30 seconds, it also dials the built-in
fallback list. That list is empty in this release
(`FALLBACK_BOOTNODES` in `bins/maya2c-node/src/bootnode_fallback.rs`). Empty
on purpose: an entry names a live host, and a stale one dials a stranger on
every start. Adding an entry is a release decision.

Operators: list at least three bootnodes, on at least two providers, so that
losing one provider does not partition a validator from the network. For
example, one at a home PC, one on a cloud VM, and one through a `wss` tunnel:

```toml
[network]
bootnodes = [
  "/dns4/seed-a.example.net/tcp/443/wss/p2p/<peer id A>",
  "/dns4/seed-b.example.net/tcp/31100/p2p/<peer id B>",
  "/ip4/203.0.113.7/tcp/31100/p2p/<peer id C>",
]
```

The addresses above are placeholders. Use your own peers' published ids.

Check the start log:

- `dialed bootnode <addr>` means the dial succeeded.
- `could not dial <addr>` is normal while that peer is down. It is retried.
- `bootnodes:   none reachable; dialing the built-in fallback list` means the
  node has had no peer for 30 seconds. It prints only when the fallback list
  is non-empty. A node with no peers and an empty list is still alone: check
  the bootnode addresses and their peer ids.
