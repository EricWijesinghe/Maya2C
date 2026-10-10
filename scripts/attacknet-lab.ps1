# Attacknet Lab PowerShell Scripts

$ErrorActionPreference = "Stop"

# Lab configuration
$LAB_ID = "maya2c-attacknet-lab-v1"
$CHAIN_ID = "maya2c-attacknet-lab"
$BASE_DIR = "D:\Maya2C-attacknet"
$REPO_DIR = "D:\Maya2C"
$REPORTS_DIR = "$REPO_DIR\reports\attacknet"

function Check-LabPrerequisites {
    Write-Host "Checking lab prerequisites..." -ForegroundColor Cyan

    # Check Rust toolchain
    $rustVersion = rustc --version
    if (-not $rustVersion.Contains("nightly-2026-07-15")) {
        Write-Warning "Rust toolchain may not be pinned to nightly-2026-07-15: $rustVersion"
    }

    # Check commit charge
    $freeCommitGB = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory / 1MB, 1)
    if ($freeCommitGB -lt 3.5) {
        Write-Warning "Low commit charge: $freeCommitGB GB free (need 3.5 GB)"
        Write-Host "Pausing live testnet peers 7-8..." -ForegroundColor Yellow
        $paused = Get-CimInstance Win32_Process -Filter "Name = 'maya2c-peer.exe'" |
            Where-Object { $_.CommandLine -match 'peer[78]\\data' }
        $paused | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
        Start-Sleep -Seconds 5
        $freeCommitGB = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory / 1MB, 1)
    }

    # Check disk space
    $freeD = (Get-PSDrive D).Free / 1GB
    if ($freeD -lt 20) {
        throw "Insufficient disk space on D:: $freeD GB free (need 20 GB)"
    }

    Write-Host "Prerequisites check passed. Commit: $freeCommitGB GB free, Disk D:: $freeD GB free" -ForegroundColor Green
}

function Provision-Lab {
    Write-Host "Provisioning attacknet lab..." -ForegroundColor Cyan

    # Create directories
    $dirs = @(
        "$BASE_DIR",
        "$BASE_DIR-genesis",
        "$BASE_DIR-v1",
        "$BASE_DIR-v2",
        "$BASE_DIR-v3",
        "$BASE_DIR-v4",
        "$BASE_DIR-bootnode",
        "$BASE_DIR-adversary",
        "$BASE_DIR-gateway",
        "$BASE_DIR-control",
        "$REPORTS_DIR",
        "$REPORTS_DIR\runs"
    )

    foreach ($dir in $dirs) {
        if (-not (Test-Path $dir)) {
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            Write-Host "Created: $dir"
        }
    }

    # Generate genesis and keys using cargo xtask
    Write-Host "Generating genesis and validator keys..." -ForegroundColor Cyan
    Set-Location $REPO_DIR
    cargo xtask attacknet --rounds 1 --weighted 2>&1 | Select-Object -First 20

    Write-Host "Lab provisioned successfully" -ForegroundColor Green
}

function Start-Lab {
    Write-Host "Starting attacknet lab..." -ForegroundColor Cyan

    Check-LabPrerequisites

    # Build binaries
    Write-Host "Building binaries..." -ForegroundColor Cyan
    Set-Location $REPO_DIR
    cargo build --release -p maya2c-node -p maya-api-gateway 2>&1 | tail -5

    # Start bootnode
    Write-Host "Starting bootnode..." -ForegroundColor Cyan
    $bootnodeArgs = @(
        "--genesis", "$BASE_DIR-genesis\genesis.json",
        "--data-dir", "$BASE_DIR-bootnode",
        "--rpc-addr", "127.0.0.1:34310",
        "--p2p-port", "33310",
        "--metrics-addr", "127.0.0.1:35310",
        "--bootnode"
    )
    Start-Process -FilePath "$REPO_DIR\target\release\maya2c-node.exe" -ArgumentList $bootnodeArgs -WindowStyle Hidden

    Start-Sleep -Seconds 3

    # Start validators
    $validators = @(
        @{ Name = "validator-1"; P2P = 33300; RPC = 34300; Metrics = 35300; Data = "$BASE_DIR-v1"; Affinity = 0x7 },
        @{ Name = "validator-2"; P2P = 33301; RPC = 34301; Metrics = 35301; Data = "$BASE_DIR-v2"; Affinity = 0x38 },
        @{ Name = "validator-3"; P2P = 33302; RPC = 34302; Metrics = 35302; Data = "$BASE_DIR-v3"; Affinity = 0x1C0 },
        @{ Name = "validator-4"; P2P = 33303; RPC = 34303; Metrics = 35303; Data = "$BASE_DIR-v4"; Affinity = 0xE00 }
    )

    foreach ($v in $validators) {
        Write-Host "Starting $($v.Name)..." -ForegroundColor Cyan
        $args = @(
            "--genesis", "$BASE_DIR-genesis\genesis.json",
            "--data-dir", $v.Data,
            "--rpc-addr", "127.0.0.1:$($v.RPC)",
            "--p2p-port", $v.P2P,
            "--validator-key", "$($v.Data)\validator.key",
            "--metrics-addr", "127.0.0.1:$($v.Metrics)"
        )

        # Add bootnodes
        $args += @("--bootnode", "/ip4/127.0.0.1/tcp/33310")
        foreach ($other in $validators) {
            if ($other.Name -ne $v.Name) {
                $args += @("--bootnode", "/ip4/127.0.0.1/tcp/$($other.P2P)")
            }
        }

        $proc = Start-Process -FilePath "$REPO_DIR\target\release\maya2c-node.exe" -ArgumentList $args -WindowStyle Hidden -PassThru
        Write-Host "$($v.Name) started (PID: $($proc.Id))"
    }

    # Wait for consensus
    Write-Host "Waiting for consensus..." -ForegroundColor Yellow
    $timeout = 120
    $start = Get-Date
    while ((Get-Date) - $start).TotalSeconds -lt $timeout {
        $heights = @()
        foreach ($v in $validators) {
            try {
                $resp = Invoke-RestMethod -Uri "http://127.0.0.1:$($v.RPC)/get_tip_height" -Method Post -Body '{"jsonrpc":"2.0","id":1,"method":"get_tip_height","params":[]}' -ContentType "application/json" -ErrorAction Stop
                if ($resp.result) { $heights += $resp.result }
            } catch {}
        }
        if ($heights.Count -eq 4 -and ($heights | Measure-Object -Minimum).Minimum -ge 3) {
            Write-Host "Consensus reached at height $($heights | Measure-Object -Minimum).Minimum" -ForegroundColor Green
            break
        }
        Start-Sleep -Seconds 2
    }

    # Start RPC gateway
    Write-Host "Starting RPC gateway..." -ForegroundColor Cyan
    $gatewayArgs = @(
        "--config", "$BASE_DIR-gateway\config.toml"
    )
    Start-Process -FilePath "$REPO_DIR\target\release\maya-api-gateway.exe" -ArgumentList $gatewayArgs -WindowStyle Hidden

    # Start monitoring stack (if Docker available)
    if (Get-Command docker -ErrorAction SilentlyContinue) {
        Write-Host "Starting monitoring stack..." -ForegroundColor Cyan
        Set-Location $REPO_DIR
        docker-compose -f docker-compose.monitoring.yml up -d
    }

    Write-Host "Lab started successfully!" -ForegroundColor Green
    Write-Host "Validator RPC endpoints:"
    foreach ($v in $validators) { Write-Host "  $($v.Name): http://127.0.0.1:$($v.RPC)" }
    Write-Host "RPC Gateway: http://127.0.0.1:34310"
    Write-Host "Prometheus: http://127.0.0.1:35310"
    Write-Host "Grafana: http://127.0.0.1:35311 (admin/lab)"
}

function Stop-Lab {
    Write-Host "Stopping attacknet lab..." -ForegroundColor Cyan

    # Stop validators and infrastructure
    $processes = @("maya2c-node", "maya-api-gateway", "attacknet-adversary")
    foreach ($procName in $processes) {
        $procs = Get-Process -Name $procName -ErrorAction SilentlyContinue
        foreach ($p in $procs) {
            # Verify it's our lab process
            $cmdLine = $p.CommandLine
            if ($cmdLine -and ($cmdLine -like "*attacknet*" -or $cmdLine -like "*$BASE_DIR*")) {
                Write-Host "Stopping $procName (PID: $($p.Id))..."
                Stop-Process -Id $p.Id -Force
            }
        }
    }

    # Stop monitoring stack
    if (Get-Command docker -ErrorAction SilentlyContinue) {
        Set-Location $REPO_DIR
        docker-compose -f docker-compose.monitoring.yml down
    }

    Write-Host "Lab stopped" -ForegroundColor Green
}

function Emergency-Down {
    Write-Warning "EMERGENCY TERMINATION of attacknet lab!"

    $processes = @("maya2c-node", "maya-api-gateway", "attacknet-adversary")
    foreach ($procName in $processes) {
        Get-Process -Name $procName -ErrorAction SilentlyContinue | Stop-Process -Force
    }

    if (Get-Command docker -ErrorAction SilentlyContinue) {
        Set-Location $REPO_DIR
        docker-compose -f docker-compose.monitoring.yml down -v
    }

    Write-Host "Emergency termination complete" -ForegroundColor Red
}

function Show-Status {
    Write-Host "=== Attacknet Lab Status ===" -ForegroundColor Cyan
    Write-Host "Lab ID: $LAB_ID"
    Write-Host "Chain ID: $CHAIN_ID"
    Write-Host ""

    Write-Host "Validators:"
    $validators = @(
        @{ Name = "validator-1"; P2P = 33300; RPC = 34300; Metrics = 35300 },
        @{ Name = "validator-2"; P2P = 33301; RPC = 34301; Metrics = 35301 },
        @{ Name = "validator-3"; P2P = 33302; RPC = 34302; Metrics = 35302 },
        @{ Name = "validator-4"; P2P = 33303; RPC = 34303; Metrics = 35303 }
    )

    foreach ($v in $validators) {
        $running = $false
        try {
            $resp = Invoke-RestMethod -Uri "http://127.0.0.1:$($v.RPC)/health" -Method Get -TimeoutSec 2 -ErrorAction Stop
            if ($resp) { $running = $true }
        } catch {}

        $status = if ($running) { "RUNNING" } else { "STOPPED" }
        $color = if ($running) { "Green" } else { "Red" }
        Write-Host "  $($v.Name): $status" -ForegroundColor $color
    }

    Write-Host ""
    Write-Host "Infrastructure:"
    $infra = @(
        @{ Name = "bootnode"; Port = 34310 },
        @{ Name = "rpc-gateway"; Port = 34310 },
        @{ Name = "adversarial-peer"; Port = 34320 }
    )

    foreach ($i in $infra) {
        $running = $false
        try {
            $resp = Invoke-RestMethod -Uri "http://127.0.0.1:$($i.Port)/health" -Method Get -TimeoutSec 2 -ErrorAction Stop
            if ($resp) { $running = $true }
        } catch {}

        $status = if ($running) { "RUNNING" } else { "STOPPED" }
        $color = if ($running) { "Green" } else { "Red" }
        Write-Host "  $($i.Name): $status" -ForegroundColor $color
    }

    Write-Host ""
    Write-Host "Resources:"
    $mem = Get-CimInstance Win32_OperatingSystem
    $freeMem = [math]::Round($mem.FreePhysicalMemory / 1MB, 1)
    $totalMem = [math]::Round($mem.TotalVisibleMemorySize / 1MB, 1)
    Write-Host "  Memory: $freeMem MB free / $totalMem MB total"

    $freeCommit = [math]::Round($mem.FreeVirtualMemory / 1MB, 1)
    Write-Host "  Commit charge: $freeCommit MB free"

    $disk = Get-PSDrive D
    Write-Host "  Disk D:: $([math]::Round($disk.Free/1GB,1)) GB free / $([math]::Round($disk.Used/1GB,1)) GB used"
}

function Run-Scenario {
    param(
        [Parameter(Mandatory=$true)][string]$ScenarioId,
        [int]$Rounds = 1
    )

    Write-Host "Running scenario $ScenarioId ($Rounds rounds)..." -ForegroundColor Cyan

    # This would use the attacknet-controller binary once built
    # For now, run the existing attacknet via cargo xtask
    Set-Location $REPO_DIR
    cargo xtask attacknet --rounds $Rounds --weighted
}

# Main
param(
    [ValidateSet("provision", "start", "stop", "down", "status", "scenario", "run-schedule")]
    [string]$Action = "status",
    [string]$ScenarioId,
    [int]$Rounds = 1,
    [int]$Week
)

switch ($Action) {
    "provision" { Provision-Lab }
    "start" { Start-Lab }
    "stop" { Stop-Lab }
    "down" { Emergency-Down }
    "status" { Show-Status }
    "scenario" {
        if (-not $ScenarioId) { throw "ScenarioId required for scenario action" }
        Run-Scenario -ScenarioId $ScenarioId -Rounds $Rounds
    }
    "run-schedule" {
        if (-not $Week) { throw "Week required for run-schedule action" }
        Write-Host "Running schedule for week $Week..." -ForegroundColor Cyan
        # Would implement schedule runner
    }
    default { throw "Unknown action: $Action" }
}