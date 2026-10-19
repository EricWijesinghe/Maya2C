# Disaster-recovery drill, on a copy (queue item 4, 2026-10-19).
#
# Question: if validator v$Validator's disk died, could it be rebuilt from
# what survives elsewhere -- its sealed key, the passphrase on E:, and a
# peer's snapshot -- and how long would it take?
#
# What it never does: start a validator. A second live copy of a validator key
# double-signs, which is the one thing worse than the outage. So the key is
# proven to open (a staking transaction signed and NOT broadcast), and the
# chain is rebuilt by a keyless observer under its own name and ports. The
# live validators, the watchdog and chain-health are not touched.
param([int]$Validator = 4, [int]$TimeoutMin = 30)

$ErrorActionPreference = 'Stop'
$drill = 'D:\Maya2C-drill'
$pass = 'E:\Maya2C-secrets\validator.passphrase'
$src = "D:\Maya2C-validators\v$Validator"
$peer = "http://127.0.0.1:$(18600 + $Validator)"
$seed = 'http://127.0.0.1:8545'
$wallet = 'D:\Maya2C\target\debug\l1-wallet.exe'
$log = "D:\Maya2C-ops\dr-drill-$(Get-Date -Format yyyyMMdd-HHmm).log"

function Say([string]$m) { $line = "$(Get-Date -Format s) $m"; Write-Output $line; Add-Content $log $line }
function Tip([string]$u) {
    $b = '{"jsonrpc":"2.0","id":1,"method":"get_tip_height","params":[]}'
    try { [int64](Invoke-RestMethod -Uri $u -Method Post -ContentType 'application/json' -Body $b -TimeoutSec 3).result } catch { -1 }
}

if (Test-Path $drill) { Remove-Item -Recurse -Force $drill }
New-Item -ItemType Directory $drill | Out-Null
Say "drill v$Validator start; seed tip $(Tip $seed)"

# 1. The key: only the sealed copy and the passphrase file.
$t = [Diagnostics.Stopwatch]::StartNew()
Copy-Item "$src\validator.key.sealed" "$drill\validator.key.sealed"
$env:L1_WALLET_PASSWORD = [guid]::NewGuid().ToString()
& $wallet --keystore "$drill\wallet.key" generate | Out-Null
$env:MAYA2C_VALIDATOR_PASSPHRASE_FILE = $pass
$out = & $wallet --keystore "$drill\wallet.key" --rpc-url $seed register-validator `
    --validator-key "$drill\validator.key.sealed" --bond 1 --nonce 0 --fee 1 --no-broadcast 2>&1
$ok = $LASTEXITCODE -eq 0 -and ($out -match '^raw:')
Remove-Item env:MAYA2C_VALIDATOR_PASSPHRASE_FILE, env:L1_WALLET_PASSWORD
Say "key: sealed copy opened=$ok in $([int]$t.Elapsed.TotalSeconds)s (signed, not broadcast)"
if (-not $ok) { Say "key FAILED: $($out | Select-Object -Last 1)"; exit 1 }

# 2. The chain: a keyless node from a fresh disk, bootstrapped from a peer.
$exe = "$drill\maya2c-drill.exe"
Copy-Item 'D:\Maya2C-validators\bin4\maya2c-validator.exe' $exe
$args = "--genesis D:\Maya2C-testnet\genesis.json --data-dir `"$drill\data`" --rpc-addr 127.0.0.1:18699 " +
        "--p2p-port 31399 --bootnode /ip4/127.0.0.1/tcp/31100 --bootstrap-from $peer --catch-up-from $peer --catch-up-from $seed " +
        "--prune-depth 3600 --snapshot-interval 3600 --snapshot-depth 3600"
$t.Restart()
$p = Start-Process -FilePath $exe -ArgumentList $args -WorkingDirectory $drill -WindowStyle Hidden `
    -RedirectStandardOutput "$drill\node.log" -RedirectStandardError "$drill\node.err" -PassThru
Say "node pid $($p.Id) started from $peer"
$caught = $false
while ($t.Elapsed.TotalMinutes -lt $TimeoutMin -and -not $p.HasExited) {
    Start-Sleep -Seconds 10
    $mine = Tip 'http://127.0.0.1:18699'; $theirs = Tip $seed
    if ($mine -ge 0 -and $theirs -ge 0 -and ($theirs - $mine) -le 2) { $caught = $true; break }
}
Say "chain: caught up=$caught in $([int]$t.Elapsed.TotalSeconds)s; drill tip $(Tip 'http://127.0.0.1:18699'), seed tip $(Tip $seed)"

# 3. Stays level for two minutes: following, not just a one-off snapshot.
if ($caught) {
    Start-Sleep -Seconds 120
    $lag = (Tip $seed) - (Tip 'http://127.0.0.1:18699')
    Say "after 120s: lag $lag blocks"
}
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
Say "drill node stopped (pid $($p.Id)); data kept in $drill for inspection"
