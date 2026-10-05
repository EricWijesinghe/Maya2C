# Foundation validators: four machines, one operator (ADR-042, gate 5)

Rehearsed on maya-testnet-1 first. The mainnet genesis ceremony repeats
the same steps with mainnet keys.

## The layout, and what it survives

| Validator | Machine | Cost |
|---|---|---|
| v1 | the PC (the testnet seed) | already running |
| v2 | Oracle Cloud Always Free, Ampere A1 (2 OCPU, 12 GB, Ubuntu 24.04) | free |
| v3 | Google Cloud free tier e2-micro (1 GB + 2 GB swap, Ubuntu 24.04) | free |
| v4 | Oracle Always Free, a second A1 VM in another **fault domain** | free |

With n = 4 the chain tolerates f = 1 down:

- **Survives:** any one machine dying, the PC rebooting for Windows Update,
  a power cut or ISP outage at home, a single Oracle or Google host failing.
- **Halts, but does not fork:** the Oracle home region going down (v2 and
  v4 together), or any two machines down at once. It resumes when one
  returns.

Why not four Oracle VMs: one region outage would take three of four. A
paid VM elsewhere replaces v4 when there is money.

None of the VMs needs an inbound port: a validator dials the WebSocket
bootnode (`p2p.maya2c.dev`) outward. Leave the cloud firewalls at
"deny all inbound except SSH".

## Steps for Eric (about 30 minutes per VM)

1. **Create the accounts.** Use cloud.oracle.com ("Start for free") and
   cloud.google.com/free. Both ask for a card to verify identity. Stay
   inside Always Free / free tier and set a **budget alert of $1**, so a
   mistake is caught at once.
2. **Create the VM** with Ubuntu 24.04 and your SSH public key (never a
   password login).
   - Oracle: shape VM.Standard.A1.Flex, 2 OCPU and 12 GB.
   - Google: e2-micro in us-west1, us-central1 or us-east1 (only those
     are free), 30 GB standard disk.
3. **On the e2-micro only**, add swap:
   `sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile && echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab`
4. **Install and join** (any VM):
   `curl -fsSL https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.sh | bash`
   - The script checks the release and genesis hashes, makes the validator
     key, and starts the node as a service.
   - Watch it catch up with `journalctl -u maya2c-node -f` until it
     follows the tip.
5. **Give the VM its own small wallet.** Your main wallet key stays on
   the PC.
   `sudo -u maya2c l1-wallet --rpc-url http://127.0.0.1:8545 generate` →
   note the address.
6. **Fund it from the PC** with exactly the bond plus 1,000,000 for fees:
   `l1-wallet --rpc-url https://rpc.maya2c.dev/rpc send --to <vm address> --amount <bond + 1000000>`
7. **Register, on the VM:**
   `sudo -u maya2c l1-wallet --rpc-url http://127.0.0.1:8545 register-validator --validator-key /var/lib/maya2c/validator.key --bond <bond>`
   - The testnet seed only proposes registrations at or above
     `--min-register-bond` (10^9), so use that bond.
   - The validator joins the committee at the next epoch boundary, within
     about an hour.
8. **Restart the node as a validator**: add `--validator-key
   /var/lib/maya2c/validator.key` to `ExecStart` in
   `/etc/systemd/system/maya2c-node.service`, then
   `sudo systemctl daemon-reload && sudo systemctl restart maya2c-node`.
9. **Check it signs:** status.maya2c.dev and the explorer show the
   committee size, and `get_bft_status` on the VM shows it as a validator,
   not an observer.

## Key handling

- A validator key exists in exactly one place: its VM, mode 600, owned by
  `maya2c`. Back it up encrypted and offline. **Never run the same key on
  two machines** — that is double signing, slashed 50 % (the attacknet's
  stolen-key test shows it).
- Each VM's wallet holds only its bond and fees.
- Use separate SSH keys per provider, and turn on 2FA on both cloud
  accounts.

## When it counts

Gate 5 is met on testnet when four validators on these four machines have
committed for seven days with none halting, as measured by
status.maya2c.dev. The mainnet ceremony then repeats steps 4–9 against the
mainnet genesis.
