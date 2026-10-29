# Starts the maya-testnet-1 stack on this PC; anything already running is left alone.
# Launched at Windows logon from the Startup folder (start-testnet.cmd), and
# every five minutes by the "Maya2C testnet watchdog" scheduled task: on
# 2026-10-02 cloudflared exited after a network drop and nothing restarted it,
# so the public endpoints were dark for about 19 hours while the chain ran on.
#
# Order matters: node first (everything reads it), then gateway and faucet,
# then the Cloudflare tunnel that publishes the two HTTP services.
# Node RPC, gateway and faucet all bind 127.0.0.1: only the tunnel is public.
#
# Each service runs under a hidden `cmd /c ... >> log 2>&1`, so its log is a
# plain append-only file and nothing here has to stay alive afterwards.

$ErrorActionPreference = 'Stop'
$root = 'D:\Maya2C-testnet'
$bin  = Join-Path $root 'bin'
$logs = Join-Path $root 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
Set-Location $root

function Start-IfStopped([string]$name, [string]$command, [string]$log) {
    # Silent when running: a watchdog task re-runs this every five minutes,
    # so only a (re)start is worth a line in startup.log.
    # Match on the binary under $bin, not the name alone: the attacknet lab
    # runs its own maya2c-node.exe from target\release, and a name match let
    # the seed stay down 18 h on 2026-10-09 while the lab nodes "counted".
    # bin* covers bin2, the sealed-key build (2026-10-18): matching bin\ alone
    # would read a bin2 seed as stopped and start a second one.
    $running = Get-Process -Name $name -ErrorAction SilentlyContinue |
        Where-Object { -not $command.Contains($bin) -or $_.Path -like "$root\bin*\*" }
    if ($running) {
        return
    }
    $line = "$command >> `"$(Join-Path $logs $log)`" 2>&1"
    # The outer quotes are for cmd: with more than two quotes on a /c line it
    # strips the first and the last, which would otherwise break the exe path.
    Start-Process -FilePath 'cmd.exe' -ArgumentList "/c `"$line`"" -WorkingDirectory $root -WindowStyle Hidden
    Write-Output "$(Get-Date -Format s) $name started"
}

# --min-register-bond: this validator never proposes a validator
# registration bonding under 10^9 -- 20 days of the faucet's whole daily
# cap. One absent registration would otherwise halt this one-validator
# chain for good (ADR-039); approved operators get their bond from us.
# The seed's key is sealed (ADR-049); its passphrase lives on E:, not beside it.
# 2026-10-19: the chain moved to the Oracle VM (4 validators); this PC only
# runs the public services, reaching the VM through the SSH forwards below.
<# Start-IfStopped 'maya2c-node' `
    "`"$root\bin4\maya2c-node.exe`" --genesis genesis.json --config node-config.toml --data-dir data --rpc-addr 127.0.0.1:8545 --p2p-port 31100 --validator-key validator\validator.key.sealed --validator-passphrase-file E:\Maya2C-secrets\validator.passphrase --min-register-bond 1000000000 --ws-listen /ip4/127.0.0.1/tcp/31101/ws --snapshot-interval 3600 --snapshot-depth 3600 --prune-depth 3600" `
    'node.log' #>

# 2026-10-21: one Oracle validator + four on this PC. The services read pc1
# (RPC 8545, WS P2P 31101), so this forward carries only the status page's
# read of the Oracle validator: if it dies, status shows Oracle down and
# nothing else is affected (the old forward also carried the public RPC and
# ran out of file handles: "accept: Too many open files").
Start-IfStopped 'ssh' `
    "C:\Windows\System32\OpenSSH\ssh.exe -N -i C:\Users\EricW\.ssh\maya_oci -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o ExitOnForwardFailure=yes -L 18601:127.0.0.1:8545 ubuntu@193.122.176.219" `
    'vm-forward.log'
Start-Sleep -Seconds 5
Start-Sleep -Seconds 5

Start-IfStopped 'maya2c-gateway' `
    "`"$bin\maya2c-gateway.exe`" --node http://127.0.0.1:8545 --listen 127.0.0.1:8080 --client-ip cf-connecting-ip" `
    'gateway.log'

# Bootstrap gateway for new validators, published as bootstrap.maya2c.dev:
# the same gateway binary under its own name (so the watchdog tells the two
# apart) with --bootstrap, which adds only the snapshot and header methods,
# at a tighter per-client rate than the public API.
Start-IfStopped 'maya2c-bootstrap' `
    "`"$bin\maya2c-bootstrap.exe`" --node http://127.0.0.1:8545 --listen 127.0.0.1:8081 --client-ip cf-connecting-ip --bootstrap --rate-per-second 10 --rate-burst 20" `
    'bootstrap.log'

# The faucet's key goes from its file into this process's environment, which
# the faucet inherits; it never appears on a command line.
$env:MAYA_FAUCET_CHAIN       = 'maya-testnet-1'
$env:MAYA_FAUCET_NODE        = 'http://127.0.0.1:8545'
$env:MAYA_FAUCET_LISTEN      = '127.0.0.1:8090'
# The tunnel is the only route to 8090, and it sets X-Forwarded-For.
$env:MAYA_FAUCET_TRUST_PROXY = 'true'
# Every grant is journaled and replayed at start, so a restart does not hand
# out a fresh daily quota.
New-Item -ItemType Directory -Force -Path (Join-Path $root 'faucet-data') | Out-Null
$env:MAYA_FAUCET_LEDGER      = Join-Path $root 'faucet-data\grants.journal'
# One grant must pay for several transfers: a hybrid-signed transfer is about
# 13 KB, so its fee is about 26,500 at the Standard tier (base fee 1).
$env:MAYA_FAUCET_DISPENSE   = '200000'
$env:MAYA_FAUCET_DAILY_CAP  = '50000000'
# The "Join the testnet" page on maya2c.dev calls the faucet from the browser.
$env:MAYA_FAUCET_CORS_ORIGINS = 'https://maya2c.dev,http://localhost:4321'
foreach ($entry in Get-Content (Join-Path $root 'secrets\faucet.env')) {
    if ($entry -match '^\s*(MAYA_FAUCET_KEY)=(.+)$') {
        Set-Item -Path "env:$($Matches[1])" -Value $Matches[2].Trim()
    }
}
Start-IfStopped 'maya-faucet' "`"$bin\maya-faucet.exe`"" 'faucet.log'
Remove-Item env:MAYA_FAUCET_KEY -ErrorAction SilentlyContinue

# Maya Chat relay over WebSocket, published as chat.maya2c.dev through the
# tunnel. Its libp2p key lives in chat-relay\relay.key: keep that folder,
# or the peer id every chat app dials changes.
Start-IfStopped 'maya-chat' `
    "`"$bin\maya-chat.exe`" --home `"$root\chat-relay`" relay --listen /ip4/127.0.0.1/tcp/4002/ws" `
    'chat-relay.log'

# Block explorer, published as explorer.maya2c.dev through the tunnel. Its
# index is in memory and rebuilt from the node at start: no database to keep.
Start-IfStopped 'maya-explorer' `
    "`"$bin\maya-explorer.exe`" --node-rpc http://127.0.0.1:8545 --listen 127.0.0.1:3000 --assets `"$root\explorer-assets`"" `
    'explorer.log'

# Status page and uptime API, published as status.maya2c.dev. It reads the
# chain's own block timestamps from the node, so it keeps no state of its
# own: a restart re-reads history in seconds.
Start-IfStopped 'maya-status' `
    "`"$bin\maya-status.exe`" --node http://127.0.0.1:18601 --listen 127.0.0.1:3100 --watch oracle-v1=http://127.0.0.1:18601 --known oracle-v1=12D3KooWJqH5AUQ1zPkMke18rcc8VHN2TWQ5XudGX3tP8zPLuyjB --watch pc1=http://127.0.0.1:8545 --known pc1=12D3KooWHeSYWgY3brFBGTqxAn41dZ7mwHQJNs116EnqXqtgQ1T1 --watch pc2=http://127.0.0.1:18702 --known pc2=12D3KooWEJcrPhuqViaLfoYKsncNBvT4325kctwok6ArxhvsJRvs --watch pc3=http://127.0.0.1:18703 --known pc3=12D3KooWKqzecLKUY9VaNgrrmiua58CQ81q9ZeZxpmK7FFTQnQLY --watch pc4=http://127.0.0.1:18704 --known pc4=12D3KooWP5Q7SDNb8uioX32rsiEjWDMcRP2JkHtSHLXfxmV3p9tu" `
    'status.log'

$tunnelConfig = Join-Path $root 'cloudflared.yml'
if (Test-Path $tunnelConfig) {
    Start-IfStopped 'cloudflared' `
        "`"C:\Program Files (x86)\cloudflared\cloudflared.exe`" tunnel --config `"$tunnelConfig`" run" `
        'tunnel.log'
}

# Project-run validators v2..v4 on this PC (ADR-042 interim; registered
# 2026-10-06 at height ~143,040, in the committee from epoch boundary
# 144,000). With n = 4 the chain survives one of them down; two down halts
# it, so the watchdog restarts any that stopped. Their keys and wallets are
# in D:\Maya2C-validators (owner-only).
$validators = 'D:\Maya2C-validators\pc\start-pc-validators.ps1'
if (Test-Path $validators) {
    & powershell -NoProfile -ExecutionPolicy Bypass -File $validators
}
