# Hybrid testnet: one Oracle validator, four on the PC

Scripts as deployed (ADR-057). Paths are the live ones on Eric's PC and are not
parameterized.

| File | Lives at | Role |
|---|---|---|
| `start-testnet.ps1` | `D:\Maya2C-testnet\` | Watchdog (every 5 min): ssh forward, services, tunnel, PC peers |
| `start-pc-validators.ps1` | `D:\Maya2C-validators\pc\` | Starts and heals pc1..pc4 |
| `oracle-watch.sh` | `/usr/local/bin/maya2c-watch.sh` on the VM | Timer: restart v1 if its round is flat |
| `make-genesis-1-plus-4.py` | any | Builds the 5-validator genesis from the 8-validator one |

No secrets are in these files: passphrases are read from `E:\Maya2C-secrets`,
the faucet key from `secrets\faucet.env`. Keep it that way.
