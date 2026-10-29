# Runbook: genesis ceremony (mainnet gate 5)

`bins/genesis-ceremony`. One coordinator, N participants on their own
machines. No secret key ever leaves the machine it was made on.

## Each participant, on their own machine

1. `genesis-ceremony contribute --label <name> --out-dir <fresh dir>`: makes a
   root key and keeps the secret local. Send the `<name>.public` file and
   note the `commitment` the tool prints.
2. `maya2c-node --generate-validator-key <path>`: send the public key it
   prints, and nothing else.
3. `l1-wallet --keystore <path> generate`: the operator address that will hold
   the bond. Send the address.

## Each council member, on their own device (ADR-050)

Three devices by default: this PC, the MacBook, and an offline backup. Every approval names its chain (`--genesis`), so approvals for one chain never count on another (ADR-050). Still, give each chain its own keys.

- Put a long random passphrase in a file on another drive than the key, and a
  paper copy somewhere safe. Never type it on a command line.
- `genesis-ceremony council key --label <pc|mac|paper> --passphrase-file <file>
  --out-dir <fresh dir>`: writes `<label>.council.sealed` (keep it) and
  `<label>.council.public` (send it). Note the `commitment` it prints.

## Coordinator

4. Collect every `*.public` into one directory. Write `validators.txt`, one
   `label key operator bond` line per validator; at least 4 lines for a
   value-bearing chain id.
5. ```
   genesis-ceremony assemble --chain-id <id> --supply <units> \
       --contributions <dir> --validators validators.txt \
       --timestamp <launch unix time> --out-dir coord        --council <dir of *.council.public> --council-threshold 2        --council-max-pause-blocks <Eric's number>
   ```
   A value-bearing chain id refuses to assemble without `--council`, and the
   tool refuses a council directory that holds a `.council.sealed` file.
   Each member checks the commitment beside their label on the sheet.
6. Publish `validators.txt`, the `*.public` files and the exact command line.

## Everyone checks

7. Each participant re-runs step 5 on their own machine from the published
   inputs. Their `genesis.json` must be **byte-identical** to the
   coordinator's (compare sha256), and the commitment beside their label must
   be the one they noted in step 1. Any difference stops the launch.
7a. The genesis id must differ from every testnet genesis ever published
   (`docs/site/authored/testnet.mdx` and the release notes). A match means a
   testnet signature would be valid on mainnet: stop.
7b. Pin it. Add the genesis id, lower-case hex, to `MAINNET_GENESIS` in
   `apps/wallet-gui/core/src/networks.rs`, merge, and tag the release from
   that commit. Until then the wallet shows no network as MAINNET and
   refuses to sign for a node that claims to be one. Publish the
   fingerprint (first and last eight hex characters) wherever the network is
   announced, so people can compare it with what their wallet shows.

## Launch

8. Pin the timestamp to the launch time. Whether nodes wait for it before
   block 1 is NOT VERIFIED: the one rehearsal with a future timestamp also had
   the connection bug below, so the two were not separated. Rehearse it
   once more before mainnet.
9. Each operator starts `maya2c-node --genesis genesis.json --validator-key
   <path> --bootnode <each other operator's p2p address>`. Starting at the
   same moment is fine. Before 2026-10-10 it was not: simultaneous dials
   collided and nodes could sit with no peers (fixed in the network layer:
   explicit dials use a fresh source port).

## Rehearsal record

2026-10-10, chain id `maya-rehearsal-2`, four participants on one machine
(this is a rehearsal of the procedure, not of independent custody):

- 4 contributions and 4 validator keys; operators bonded 10^9 each, 60-block
  epochs.
- Coordinator and independent assembly: `genesis.json` byte-identical (sha256
  `f5be0306193dd976…`). All four commitments on the sheet matched the ones
  printed at contribution. That genesis had a timestamp 2.6 h ahead. Its
  launch built no block, cause not isolated (step 8). A re-assembly with a past timestamp, launched
  below, was byte-identical between coordinator and verifier too.
- Launch, before the fix: two simultaneous starts stayed at height 0 with
  zero connections for 90 s. A third, with connection errors now logged,
  showed 20 failed dials (`Handshake failed: input error`, `os error 10048`)
  before redials got through.
- After the fix, three simultaneous-start trials: every node at height ≥ 3 in
  4 s, 0 failed connections.

## Using the brake (ADR-050)

A pause needs two of the three council devices. Nonce: the council's current
nonce (0 for the first action; it goes up by one after every action).

1. On each of two member devices:
   `genesis-ceremony council approve --key <label>.council.sealed
   --passphrase-file <file> --council <dir of the three .public files> --genesis genesis.json
   --nonce <n> --pause <module> --blocks <n>` (or `--resume <module>`).
   It writes `<label>.approval`: a signature, no secret. Modules are named as
   in `invariant_guard::Module`: `dex`, `vm`, `oracle`, and so on.
2. On any machine: `genesis-ceremony council combine --council <dir> --genesis genesis.json
   a.approval b.approval`. It checks every signature and refuses one approval,
   a repeat, a forgery or two approvals for different actions. It writes
   `council-action.hex`.
3. From any funded wallet: `l1-wallet council-action --action
   council-action.hex`. The wallet only pays the fee; the chain checks the
   approvals.

Rehearsed 2026-10-19 with the tool on one machine: three keys, a genesis with
the 2-of-3 council, two approvals, combine (13,292-character action), wrong
passphrase refused, one approval refused, sealed key in the council
directory refused. The on-chain effect is covered by
`crates/node/tests/council_pause_tests.rs`. Submitting a combined file to a
live node is NOT VERIFIED yet: the testnet has no council (ADR-050 question 4).
