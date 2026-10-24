# ADR-057 hybrid topology threat model (PRE-AUDIT, self-run, 2026-10-25)

Read-only review by a Claude subagent of ADR-057, `infra/hybrid-testnet/*` and
ADR-058. Live D:\ copies, `cloudflared.yml`, the Oracle unit/authorized_keys/
security list and the sealed-key KDF were **not** examined. Not an audit.
Items marked (V) were re-checked by me; the rest are the reviewer's reading.

| # | Sev | Finding | Cheapest mitigation (free unless noted) |
|---|---|---|---|
| 1 | Critical | Oracle holds 71.4% of stake and alone finalizes: its compromise = any finality. (V: arithmetic confirmed by the committee review) | VM hardening; structural fix is no validator above 1/3 (Eric's decision) |
| 2 | High | Oracle outage or a flood of public :31100 halts the chain (PC = 28.6% < quorum) | OCI security-list/iptables rate limit; second validator on other hardware (cost: Eric) |
| 3 | High | Watchdog wipes the data dir (empty safety log) without ADR-043's epoch check; follower x3 and 45-min rules can fire in the same epoch the node last signed. (V: ADR-043 wording confirmed, script has no check) | Wipe only if the network epoch > the node's newest `bft/epoch-N` dir. **Done in this change set.** |
| 4 | High | Snapshot bootstrap validates header linkage and PoW only, not a committee-signed checkpoint (reviewer's reading of `snapshot.rs:328-370`, `store.rs:163-200`; NOT yet verified) | Require the snapshot block id to match a committee-signed checkpoint; operationally compare manifest id to `get_checkpoint` first |
| 5 | High | SSH forward uses `accept-new`; key `maya_oci` may have a shell | Pin host key, `StrictHostKeyChecking=yes`; restrict the key with `permitopen` (touches the VM: needs APPROVED) |
| 6 | High | PC compromise: sealed keys + plaintext passphrase files on the same machine; BitLocker off; lying RPC on pc1 | BitLocker, DPAPI/Credential Manager, ACLs, rotate faucet key |
| 7 | High (cond.) | Cloudflare account control / TLS termination; faucet CORS trusts maya2c.dev | Hardware-key 2FA, scoped tokens, audit-log alerts |
| 8 | Med | Eclipse a PC peer's public p2p port to force follower x3 then wipe | Firewall allowlist; wipe on flat height, not follower status alone |
| 9 | Med | 45-min rule cannot tell slow catch-up from a hang (this happened 10-23/24) | Check height/log growth before wiping |
| 10 | Med | `data-stale` is deleted before the next move: one generation kept | Keep timestamped generations |
| 11 | Med | `Stop-Process -Force` on live validators | Graceful stop first |
| 12 | Med | Loopback services lack Host/Origin guards; gateway/faucet trust forwarded-IP headers | Host allowlist; trust XFF only from the tunnel |
| 13-17 | Low | process matching by name; Oracle timer restarts the sole finalizer under DoS; status reads Oracle only; live scripts not in git/ACL; p2p source trust | see source |
