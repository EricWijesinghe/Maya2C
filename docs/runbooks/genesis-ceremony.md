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

## Coordinator

4. Collect every `*.public` into one directory. Write `validators.txt`, one
   `label key operator bond` line per validator; at least 4 lines for a
   value-bearing chain id.
5. ```
   genesis-ceremony assemble --chain-id <id> --supply <units> \
       --contributions <dir> --validators validators.txt \
       --timestamp <launch unix time> --out-dir coord
   ```
6. Publish `validators.txt`, the `*.public` files and the exact command line.

## Everyone checks

7. Each participant re-runs step 5 on their own machine from the published
   inputs. Their `genesis.json` must be **byte-identical** to the
   coordinator's (compare sha256), and the commitment beside their label must
   be the one they noted in step 1. Any difference stops the launch.

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
