# Four PC validators of maya-testnet-1 (2026-10-20 stake-weighted genesis).
# The PC holds about 29% of the stake (4 x 1e8 against 1e9 on the one Oracle
# validator), so the chain keeps finalizing on the VM alone when this PC is off;
# these four vote and earn when it is on. Run by the watchdog every 5 min:
# it starts only the ones that are not running (matched by data dir).
param([string]$TrustedBlock = $env:MAYA2C_TRUSTED_BLOCK, [switch]$AllowSleep)
$ErrorActionPreference = 'Stop'
# The same rule the node enforces (docs/runbooks/validator-start-guards.md):
# a plan that can sleep this PC is refused, so the watchdog logs one reason
# instead of four failed starts. English labels only: on another Windows
# language this refuses, which is the safe side; check the plan by hand and pass -AllowSleep.
function Test-PlanCannotSleep {
    foreach ($alias in 'STANDBYIDLE', 'HIBERNATEIDLE') {
        $out = powercfg /query SCHEME_CURRENT SUB_SLEEP $alias 2>&1 | Out-String
        $m = [regex]::Match($out, 'Current AC Power Setting Index: 0x([0-9a-fA-F]+)')
        if (-not $m.Success -or [Convert]::ToInt64($m.Groups[1].Value, 16) -ne 0) { return $false }
    }
    return $true
}
if (-not $AllowSleep -and -not (Test-PlanCannotSleep)) {
    Write-Output "$(Get-Date -Format s) refusing PC validators: the AC power plan can sleep this PC. Fix: powercfg /change standby-timeout-ac 0 ; powercfg /change hibernate-timeout-ac 0"
    exit 1
}
$root = 'D:\Maya2C-validators\pc'
$exe = Join-Path $root 'bin\maya2c-node.exe'
$vm = '193.122.176.219'
$oracle = @(
    "/ip4/$vm/tcp/31100/p2p/12D3KooWJqH5AUQ1zPkMke18rcc8VHN2TWQ5XudGX3tP8zPLuyjB"
)
$running = Get-CimInstance Win32_Process -Filter "Name = 'maya2c-node.exe'" | ForEach-Object { $_.CommandLine }
# pc1 serves the public services (RPC 8545, WebSocket P2P 31101) so no SSH
# forward sits in their data path. Every PC node catches up from the Oracle
# validator (18601 forward, used at start only): a validator that fell behind
# does not rejoin the DAG by gossip alone.
$ws = '--ws-listen /ip4/127.0.0.1/tcp/31101/ws '
# Catch-up is latency-bound (a few blocks in flight, each a round trip), so a
# node hours behind needs an hour through the 0.7 s SSH link but minutes from
# a voting peer on this PC. Prefer those; Oracle is the fallback.
function Get-Sources([int]$self) {
    $urls = @()
    foreach ($j in 1..4) {
        if ($j -eq $self) { continue }
        $u = 'http://127.0.0.1:' + $(if ($j -eq 1) { 8545 } else { 18700 + $j })
        try {
            $r = Invoke-RestMethod $u -Method Post -ContentType 'application/json' -TimeoutSec 2 `
                -Body '{"jsonrpc":"2.0","id":1,"method":"get_bft_status","params":[]}'
            if ($r.result.committed_round -gt 0 -and -not $r.result.follower) { $urls += $u }
        } catch { }
    }
    if (-not $urls) { $urls = @('http://127.0.0.1:18601') }
    return $urls
}
# B49: a restart passes a trusted block. Order: -TrustedBlock or MAYA2C_TRUSTED_BLOCK
# (the operator's explorer block, always wins); otherwise fetched from the first
# two live peers, which must agree on the block at the lower tip. One source is a
# convenience for our own nodes, and it is warned about.
function Invoke-TrustedRpc([string]$Url, [string]$Method, $Params) {
    $body = @{ jsonrpc = '2.0'; id = 1; method = $Method; params = $Params } | ConvertTo-Json -Compress -Depth 4
    (Invoke-RestMethod $Url -Method Post -Body $body -ContentType 'application/json' -TimeoutSec 5).result
}
function Get-TrustedFlag([string[]]$Urls) {
    $pair = @($Urls | Where-Object { $_ } | Select-Object -First 2)
    if ($pair.Count -eq 0) { return $null }
    if ($pair.Count -eq 1) {
        Write-Warning "trusted block from a single source ($($pair[0])): our own nodes only. An outside operator sets MAYA2C_TRUSTED_BLOCK from the explorer, or two independent peers"
    }
    $tips = @()
    foreach ($u in $pair) {
        try { $tips += [uint64](Invoke-TrustedRpc $u 'get_chain_info' @()).height }
        catch { Write-Warning "trusted block: $u did not answer"; return $null }
    }
    $m = ($tips | Measure-Object -Minimum).Minimum
    $ids = @()
    foreach ($u in $pair) {
        try {
            $b = Invoke-TrustedRpc $u 'get_block_by_height' @($m)
            if ([uint64]$b.height -ne $m -or $b.header.id -notmatch '^[0-9a-fA-F]{64}$') { throw 'no usable block' }
            $ids += $b.header.id.ToLower()
        } catch { Write-Warning "trusted block: $u gave no usable block at $m"; return $null }
    }
    if (@($ids | Select-Object -Unique).Count -ne 1) { Write-Warning "trusted block: the sources disagree at height $m"; return $null }
    return "--trusted-block $($m):$($ids[0])"
}
# ADR-043: a wiped node restarts with an empty safety log, which is safe only
# when the network is in a later epoch than the last one the node signed in.
# Unknown network epoch counts as unsafe: a missed wipe costs time, a wrong
# one can double-sign.
function Get-NetEpoch {
    foreach ($u in @('http://127.0.0.1:18601', 'http://127.0.0.1:8545', 'http://127.0.0.1:18702')) {
        try {
            $r = Invoke-RestMethod $u -Method Post -ContentType 'application/json' -TimeoutSec 3 `
                -Body '{"jsonrpc":"2.0","id":1,"method":"get_bft_status","params":[]}'
            if ($null -ne $r.result.epoch) { return [int]$r.result.epoch }
        } catch { }
    }
    return $null
}
function Test-WipeSafe([string]$dir) {
    $net = Get-NetEpoch
    if ($null -eq $net) { return $false }
    $last = Get-ChildItem (Join-Path $dir 'data\bft') -Directory -Filter 'epoch-*' -ErrorAction SilentlyContinue |
        ForEach-Object { [int]($_.Name -replace '^epoch-', '') } | Sort-Object -Descending | Select-Object -First 1
    return ($null -eq $last) -or ($net -gt $last)
}
for ($i = 1; $i -le 4; $i++) {
    $dir = Join-Path $root "pc$i"
    # Self-heal (the laptop sleeps and moves): Oracle keeps only 8 epochs of
    # checkpoints (~14 h), so a peer further behind than that hangs silently
    # and can only restore from a snapshot. Wipe such a peer (node key kept)
    # and the bootstrap flag below restores it. "Unhealthy" = RPC does not
    # report a committed round; 45 min allows a snapshot restore to finish.
    $port = if ($i -eq 1) { 8545 } else { 18700 + $i }
    $mark = Join-Path $dir 'unhealthy.since'
    $isUp = [bool]($running | Where-Object { $_ -like "*$dir\data*" })
    $wipe = $false
    if ($isUp) {
        $ok = $false
        try {
            $r = Invoke-RestMethod "http://127.0.0.1:$port" -Method Post -ContentType 'application/json' -TimeoutSec 5 `
                -Body '{"jsonrpc":"2.0","id":1,"method":"get_bft_status","params":[]}'
            $ok = $r.result.committed_round -gt 0
            # A node whose epoch trails the network's is as good as a follower:
            # after a disk-full (os error 112) pc1/pc2 sat 6 epochs behind with
            # an RPC that still reported a round and follower=false (2026-10-25).
            $net = Get-NetEpoch
            $follower = [bool]$r.result.follower -or (($null -ne $net) -and ([int]$r.result.epoch -lt $net))
        } catch { }
        $fmark = Join-Path $dir 'follower.since'
        if ($ok -and -not $follower) {
            Remove-Item $mark, $fmark -ErrorAction SilentlyContinue; continue
        }
        if ($ok) {
            # In sync but only following (it fell out of the engine window): a
            # restart while at the tip makes it vote again (tested 2026-10-24).
            # Three restarts that do not help mean it is not really at the tip.
            Remove-Item $mark -ErrorAction SilentlyContinue
            if (-not (Test-Path $fmark)) { Set-Content $fmark "$(Get-Date -Format o),0"; continue }
            $since, $tries = (Get-Content $fmark -Raw).Trim() -split ','
            if (((Get-Date) - [datetime]$since).TotalMinutes -lt 10) { continue }
            $wipe = [int]$tries -ge 3
            Set-Content $fmark "$(Get-Date -Format o),$([int]$tries + 1)"
        } else {
            if (-not (Test-Path $mark)) { Set-Content $mark (Get-Date -Format o); continue }
            if (((Get-Date) - [datetime](Get-Content $mark -Raw)).TotalMinutes -lt 45) { continue }
            $wipe = $true
        }
        if ($wipe -and -not (Test-WipeSafe $dir)) {
            Write-Output "$(Get-Date -Format s) pc$i wipe deferred: network epoch not past the node's last"
            $wipe = $false
        }
        Get-CimInstance Win32_Process -Filter "Name = 'maya2c-node.exe'" |
            Where-Object { $_.CommandLine -like "*$dir\data*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
        Start-Sleep -Seconds 3
        $isUp = $false
    } else {
        $newest = Get-ChildItem (Join-Path $dir 'data\archive') -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        $wipe = $newest -and (((Get-Date) - $newest.LastWriteTime).TotalHours -gt 8)
        if ($wipe -and -not (Test-WipeSafe $dir)) {
            Write-Output "$(Get-Date -Format s) pc$i wipe deferred: network epoch not past the node's last"
            $wipe = $false
        }
    }
    if ($wipe) {
        $stale = Join-Path $dir 'data-stale'
        Remove-Item $stale -Recurse -Force -ErrorAction SilentlyContinue
        Move-Item (Join-Path $dir 'data') $stale
        New-Item -ItemType Directory (Join-Path $dir 'data') | Out-Null
        Copy-Item (Join-Path $stale 'node_key') (Join-Path $dir 'data\node_key')
        Remove-Item $mark -ErrorAction SilentlyContinue
        Write-Output "$(Get-Date -Format s) pc$i wiped for snapshot restore"
    } elseif ($isUp) { continue }
    # @() keeps a single Oracle bootnode an array, so += appends instead of concatenating strings.
    $boot = @($oracle | ForEach-Object { "--bootnode $_" })
    for ($j = 1; $j -le 4; $j++) {
        if ($j -ne $i) { $boot += "--bootnode /ip4/127.0.0.1/tcp/$(31400 + $j)/p2p/$(Get-Content (Join-Path $root "pc$j\peer.id") -Raw)" }
    }
    $src = Get-Sources $i
    # B49: a catch-up start passes the trusted block. Without one the node still
    # starts, and it refuses only if it is behind the unbonding window.
    $trusted = ''
    if ($src.Count -gt 0) {
        $flag = if ($TrustedBlock -match '^\d+:[0-9a-fA-F]{64}$') { "--trusted-block $TrustedBlock" } else { Get-TrustedFlag $src }
        if ($flag) { $trusted = "$flag " }
        else { Write-Output "$(Get-Date -Format s) pc$($i): no trusted block; starting without one (a node behind the window will refuse)" }
    }
    $args ="--genesis `"$root\genesis.json`" --data-dir `"$dir\data`" --rpc-addr 127.0.0.1:$(if ($i -eq 1) { 8545 } else { 18700 + $i }) --p2p-port $(31400 + $i) --allow-private-peers " +
        "--validator-key `"$dir\validator.key.sealed`" --validator-passphrase-file E:\Maya2C-secrets\pc$i.passphrase " +
        "--min-register-bond 1000000000 --snapshot-interval 3600 --snapshot-depth 3600 --prune-depth 3600 " +
        (($src | ForEach-Object { "--catch-up-from $_ " }) -join '') + $trusted + $(if (-not (Test-Path (Join-Path $dir 'data/archive'))) { "--bootstrap-from http://127.0.0.1:18601 " }) + $(if ($i -eq 1) { $ws }) + ($boot -join ' ')
    $line = "`"$exe`" $args >> `"$dir\node.log`" 2>&1"
    Start-Process -FilePath 'cmd.exe' -ArgumentList "/c `"$line`"" -WorkingDirectory $dir -WindowStyle Hidden
    Write-Output "$(Get-Date -Format s) pc$i started"
    # One catch-up at a time: peers started together left each other hung
    # (2026-10-24). Wait for this one to report a round, or 3 minutes.
    for ($w = 0; $w -lt 18; $w++) {
        Start-Sleep -Seconds 10
        try {
            $s = Invoke-RestMethod "http://127.0.0.1:$port" -Method Post -ContentType 'application/json' -TimeoutSec 3 `
                -Body '{"jsonrpc":"2.0","id":1,"method":"get_bft_status","params":[]}'
            if ($s.result.committed_round -gt 0) { break }
        } catch { }
    }
}
