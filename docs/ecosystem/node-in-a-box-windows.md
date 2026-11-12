# Node in a box: Windows

Run a Maya2C node on one Windows PC. The setup checks the machine, writes the
node's config, locks the key files, and prints the service definition for you
to apply. It installs nothing on its own.

Status: setup and doctor are built and tested (`xtask/src/node_box/`). A
node run from them is not yet a joined validator on the public network; see
the open gates in `docs/ecosystem/CATALOG.md`.

## 1. Build the node

From a checkout of the repository, in PowerShell:

```powershell
cargo build -p maya2c-node
```

Copy the built binaries to a stable folder, so a service keeps pointing at the
same file after you rebuild:

```powershell
New-Item -ItemType Directory -Force D:\Maya2C-bin | Out-Null
Copy-Item target\debug\maya2c-node.exe D:\Maya2C-bin\
$env:MAYA2C_BIN_DIR = "D:\Maya2C-bin"
```

Keep the node's data off `C:` when you can. The default directory is
`%USERPROFILE%\maya2c-node`; pass `--dir` to choose another.

## 2. Get the genesis file

Ask the network's operator for `genesis.json` and its SHA-256. Check the hash
before you use the file:

```powershell
Get-FileHash genesis.json -Algorithm SHA256
```

## 3. Set up

```powershell
cargo xtask node setup --genesis genesis.json --dir D:\Maya2C-node
```

Setup does this, in order:

1. Checks the host. A FAIL stops it before anything is written: free disk
   under 20 GiB, RAM under 4 GiB, a node port already in use, or the clock not
   synchronised. A WARN (for example, the time service cannot be read) does
   not stop it.
2. Copies the genesis file in and prints its SHA-256.
3. Writes `node-config.toml` from the node's own template. An existing file
   is kept; pass `--force` to replace it.
4. Secures the key files that exist in the directory (`validator.key`,
   `validator.key.sealed`, `validator.passphrase`). Only your account and
   SYSTEM keep access. Inheritance is removed afterwards.
5. Prints a `schtasks` command. Read it, then run it yourself in an elevated
   PowerShell if you want the node to start at boot.

If you have a validator key, create it first with
`maya2c-node --generate-validator-key D:\Maya2C-node\validator.key`, then run
setup. Without one the node runs as an observer.

## 4. Check it

```powershell
cargo xtask node doctor --dir D:\Maya2C-node
```

Doctor checks the config sections, the genesis hash, each key file's
permissions, that the node's RPC answers with a height, and the host again.
It exits nonzero on any FAIL.

## If a key shows as open

Setup prints a WARN when a grant survives (for example `BUILTIN\Administrators`
with an explicit entry). Remove it by hand:

```powershell
icacls D:\Maya2C-node\validator.key /remove:g "BUILTIN\Administrators"
```

Then run `cargo xtask node doctor` again.

## Ports

The node's defaults are p2p 30333 and RPC 8545. The live networks on the same
PC may hold other ports. Pass `--p2p-port` and `--rpc-port` to setup and
doctor to match what you configure.
