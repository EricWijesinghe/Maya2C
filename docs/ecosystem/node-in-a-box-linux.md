# Node in a box: Linux

Run a Maya2C node on one Linux machine under systemd. The setup checks the
machine, writes the node's config, sets the key files to mode 0600, and prints
a systemd unit for you to install. It installs nothing on its own.

Status: setup and doctor are built and tested (`xtask/src/node_box/`). A
node run from them is not yet a joined validator on the public network; see
the open gates in `docs/ecosystem/CATALOG.md`.

## 1. Build the node

From a checkout of the repository:

```sh
cargo build -p maya2c-node
sudo install -m 0755 target/debug/maya2c-node /usr/local/bin/maya2c-node
```

Setup reads the binary from `target/debug` unless `MAYA2C_BIN_DIR` is set.
Export `MAYA2C_BIN_DIR=/usr/local/bin` before running setup, so the printed
unit points at the installed binary.

Use a dedicated service account rather than your login:

```sh
sudo useradd --system --home /var/lib/maya2c --shell /usr/sbin/nologin maya
sudo install -d -o maya -g maya -m 0700 /var/lib/maya2c/node
```

## 2. Get the genesis file

Ask the network's operator for `genesis.json` and its SHA-256, then check it:

```sh
sha256sum genesis.json
```

## 3. Set up

Run setup as the account that will own the files (the service account, when
the directory is `/var/lib/maya2c/node`):

```sh
cargo xtask node setup --genesis genesis.json --dir /var/lib/maya2c/node
```

Setup does this, in order:

1. Checks the host. A FAIL stops it before anything is written: free disk
   under 20 GiB on the volume holding the directory, RAM under 4 GiB, a node
   port already in use, or the clock not synchronised (`timedatectl`). A WARN
   (for example, `timedatectl` is missing) does not stop it.
2. Copies the genesis file in and prints its SHA-256.
3. Writes `node-config.toml` from the node's own template. An existing file
   is kept; pass `--force` to replace it.
4. Runs `chmod 600` on each key file that exists (`validator.key`,
   `validator.key.sealed`, `validator.passphrase`).
5. Prints a systemd unit. Read it before you install it.

If you have a validator key, create it first with
`maya2c-node --generate-validator-key /var/lib/maya2c/node/validator.key`.
Without one the node runs as an observer.

## 4. Install the service

Copy the printed unit to a file, then:

```sh
sudo cp maya2c-node.service /etc/systemd/system/maya2c-node.service
sudo systemctl daemon-reload
sudo systemctl enable --now maya2c-node
journalctl -u maya2c-node -f
```

The printed unit runs as the account setup was run under (`$USER`). Edit
`User=` if you use a different service account, and make sure that account
can read the directory.

## 5. Check it

```sh
cargo xtask node doctor --dir /var/lib/maya2c/node
```

Doctor checks the config sections, the genesis hash, each key file's mode (a
group or other bit is a FAIL), that the node's RPC answers with a height, and
the host again. It exits nonzero on any FAIL.

## Ports

The node's defaults are p2p 30333 and RPC 8545. Pass `--p2p-port` and
`--rpc-port` to setup and doctor if you configure others. The RPC should stay
on 127.0.0.1; put a gateway in front of it rather than opening it.
