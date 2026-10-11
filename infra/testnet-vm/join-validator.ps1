<#
.SYNOPSIS
    Join a running Maya2C testnet as a validator on a Windows machine.

.DESCRIPTION
    The Windows counterpart of infra/testnet-vm/install.sh "join mode". Run once
    as Administrator; it installs the maya2c-node binary, fetches and verifies
    the network's genesis, generates a validator key, and registers a background
    task that starts the node at boot and keeps it running.

    THIS SCRIPT IS DELIBERATELY NOT STEALTHY, AND THAT IS A FEATURE.
    A node operator deploying to machines they own has nothing to hide; software
    that hides what it is doing from the people whose machines it runs on is, by
    definition, malware. So this installer:
      * announces exactly what it installs, and refuses to run with -Silent
        unless the caller also passes -IAmTheMachineOwner;
      * uses a clearly named, discoverable mechanism: a Scheduled Task called
        "Maya2C Validator", visible in Task Scheduler and in the process list as
        maya2c-node.exe;
      * writes a log you can read at  C:\ProgramData\Maya2C\logs\node.log;
      * ships its own uninstaller:  join-validator.ps1 -Uninstall;
      * never disables antivirus, never obfuscates, never hides a window by
        pretending to be something else. Running headless as SYSTEM has no
        window because there is no interactive session, not because anything is
        concealed.

    100 machines run by one person are still ONE operator: excellent for
    liveness and stress testing, but not "independent validators" and not
    gate 4. Record them honestly as single-operator nodes.

.PARAMETER GenesisUrl
    Where the seed operator published genesis.json.

.PARAMETER GenesisSha256
    The sha256 the seed operator published alongside genesis.json. A wrong file
    is a different chain, so this is required: the installer refuses a genesis
    whose hash does not match.

.PARAMETER Bootnode
    The seed's libp2p address, e.g. /ip4/203.0.113.7/tcp/31100 .

.PARAMETER ReleaseTag
    Which tagged release to install the node binary from. Defaults to the
    newest node-v* prerelease.

.PARAMETER NodeSha256
    Optional: pin the expected sha256 of the node archive. When omitted, the
    archive is verified against the SHA256SUMS file published in the same
    release (transfer integrity).

.PARAMETER Uninstall
    Stop and remove the task, the binary and the data directory, then exit.

.EXAMPLE
    # From an elevated PowerShell, on each machine the operator owns:
    .\join-validator.ps1 -GenesisUrl https://testnet.maya2c.dev/genesis.json `
        -GenesisSha256 <hash> -Bootnode /ip4/<seed-ip>/tcp/31100

.NOTES
    Not code-signed, so SmartScreen may warn about an unknown publisher: that is
    expected for an unsigned prerelease, and the sha256 checks are what make the
    download trustworthy, not a signature.
#>
[CmdletBinding()]
param(
    # The three network values default to the live maya-testnet-1 settings,
    # kept in step with scripts/join-testnet.sh (the Linux join path). Someone
    # helping run the testnet needs only  -Silent -IAmTheMachineOwner ; pass
    # these explicitly only to point the installer at a different network.
    [string]$GenesisUrl    = 'https://maya2c.dev/testnet/genesis.json',
    [string]$GenesisSha256 = 'dd9bb356c94b7699e6e8d2595681faee48be435b02dc927677cbf833e55edd5d',
    [string]$Bootnode      = '/dns4/p2p.maya2c.dev/tcp/443/wss/p2p/12D3KooWK2bykmqyzdK4TMSTBQ5visSjkFoK8wj88aMn3ms4ByiG',
    # Snapshot/catch-up endpoint so a joining node loads a recent snapshot over
    # https (port 443, NAT-friendly) instead of replaying the chain from block 0.
    [string]$Bootstrap     = 'https://bootstrap.maya2c.dev/rpc',
    [string]$ReleaseTag,
    [string]$NodeSha256,
    [switch]$Silent,
    [switch]$IAmTheMachineOwner,
    [switch]$Uninstall,
    [switch]$DryRun,
    # Task Scheduler priority, 0 (highest) .. 10 (idle). Default 10 so the node
    # yields all CPU to whatever is in the foreground: on a shared machine such
    # as a gaming cafe it runs flat-out only while the PC is otherwise idle and
    # steps aside the instant a game needs the cores. 7 (below-normal) keeps it
    # a little more responsive if liveness ever suffers.
    [ValidateRange(0, 10)]
    [int]$Priority = 10,
    # The libp2p TCP port the node listens on. Default 31100 (install.sh's
    # P2P_BASE). One node per machine, so the default is right for a cafe PC;
    # override it only to run a second node on a machine that already has one —
    # e.g. testing a joiner on the seed operator's own PC, where the seed
    # already holds 31100 and two nodes cannot share it.
    [ValidateRange(1, 65535)]
    [int]$P2pPort = 31100,
    # The loopback RPC port. Also must differ for a second node on one machine.
    [ValidateRange(1, 65535)]
    [int]$RpcPort = 32000
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# --- Constants: one place, no magic numbers scattered through the body. ------
$Repo        = 'EricWijesinghe/Maya2C'
$InstallDir  = Join-Path $env:ProgramFiles 'Maya2C'
$DataDir     = Join-Path $env:ProgramData  'Maya2C'
$LogDir      = Join-Path $DataDir 'logs'
$NodeExe     = Join-Path $InstallDir 'maya2c-node.exe'
$GenesisPath = Join-Path $DataDir 'genesis.json'
$KeyPath     = Join-Path $DataDir 'validator.key'
$NodeDataDir = Join-Path $DataDir 'v0'
$TaskName    = 'Maya2C Validator'
$ServiceDesc = 'Maya2C testnet validator (single-operator node). Logs: ' + $LogDir
$WinTarget   = 'maya2c-x86_64-pc-windows-msvc'
$RpcAddr     = "127.0.0.1:$RpcPort" # RPC is unauthenticated; bind loopback only.
# $P2pPort and $RpcPort come from params so a second node can coexist on one PC.

function Write-Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Write-Note($m) { Write-Host "    $m" -ForegroundColor DarkGray }
function Die($m)        { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

function Assert-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p  = New-Object Security.Principal.WindowsPrincipal($id)
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltinRole]::Administrator)) {
        Die 'Run this from an elevated PowerShell (Administrator). It installs a boot task.'
    }
}

function Remove-Install {
    Write-Step 'Uninstalling Maya2C validator'
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask    -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Note "removed scheduled task '$TaskName'"
    }
    Get-Process maya2c-node -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    foreach ($d in @($InstallDir, $DataDir)) {
        if (Test-Path $d) { Remove-Item $d -Recurse -Force; Write-Note "removed $d" }
    }
    Write-Host 'Uninstalled. The machine is clean.' -ForegroundColor Green
    exit 0
}

function Show-Consent {
    Write-Host ''
    Write-Host '  Maya2C testnet validator installer' -ForegroundColor White
    Write-Host '  -----------------------------------' -ForegroundColor White
    Write-Host '  This will, on THIS machine:' -ForegroundColor White
    Write-Note "install maya2c-node.exe to  $InstallDir"
    Write-Note "store chain data and keys in $DataDir"
    Write-Note "register a boot task '$TaskName' (visible in Task Scheduler)"
    Write-Note "run the node headless as SYSTEM (no window) at idle priority $Priority"
    Write-Note "  -> yields CPU to games/foreground apps; logs at $LogDir\node.log"
    Write-Note "listen on p2p port $P2pPort, rpc 127.0.0.1:$RpcPort (loopback)"
    Write-Note "connect to the testnet at bootnode $Bootnode"
    Write-Note "catch up from snapshot endpoint $Bootstrap"
    Write-Host '  It is a TESTNET node. It carries no value. Remove any time with' -ForegroundColor White
    Write-Host '      .\join-validator.ps1 -Uninstall' -ForegroundColor White
    Write-Host ''
    if ($Silent) {
        if (-not $IAmTheMachineOwner) {
            Die 'Refusing a silent install without -IAmTheMachineOwner. The person whose machine this is must know a validator runs on it. This installer will not hide itself.'
        }
        Write-Note 'silent install authorised by -IAmTheMachineOwner'
        return
    }
    $ans = Read-Host 'Install the Maya2C validator on this machine? [y/N]'
    if ($ans -notmatch '^[Yy]') { Die 'Cancelled by operator.' }
}

function Get-ReleaseTag {
    if ($ReleaseTag) { return $ReleaseTag }
    Write-Step 'Finding newest node-v* release'
    $rel = Invoke-RestMethod "https://api.github.com/repos/$Repo/releases" -Headers @{ 'User-Agent' = 'maya2c-join' }
    $tag = ($rel | Where-Object { $_.tag_name -like 'node-v*' } | Select-Object -First 1).tag_name
    if (-not $tag) { Die 'No node-v* release found. Pass -ReleaseTag explicitly.' }
    Write-Note "using $tag"
    return $tag
}

function Get-Sha256($path) { (Get-FileHash $path -Algorithm SHA256).Hash.ToLower() }

function Install-Node($tag) {
    $base    = "https://github.com/$Repo/releases/download/$tag"
    $archive = "$WinTarget.tar.gz"
    $tmp     = Join-Path $env:TEMP "maya2c-$([guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    $arcPath = Join-Path $tmp $archive

    Write-Step "Downloading $archive from $tag"
    Invoke-WebRequest "$base/$archive" -OutFile $arcPath -UseBasicParsing

    # Verify the download: an explicit pin if given, else the release's own
    # SHA256SUMS. A node binary that fails its checksum is never run.
    $got = Get-Sha256 $arcPath
    if ($NodeSha256) {
        if ($got -ne $NodeSha256.ToLower()) { Die "node archive sha256 mismatch: got $got" }
        Write-Note 'sha256 matches the pinned -NodeSha256'
    } else {
        $sums = Join-Path $tmp 'SHA256SUMS'
        Invoke-WebRequest "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
        $want = (Select-String -Path $sums -Pattern ([regex]::Escape($archive)) | Select-Object -First 1).Line.Split(' ')[0].ToLower()
        if (-not $want) { Die "SHA256SUMS has no entry for $archive" }
        if ($got -ne $want) { Die "node archive sha256 mismatch: got $got want $want" }
        Write-Note 'sha256 matches the release SHA256SUMS'
    }

    Write-Step "Extracting to $InstallDir"
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    tar -xzf $arcPath -C $tmp                       # bsdtar ships with Windows 10+
    Copy-Item (Join-Path $tmp "$WinTarget\maya2c-node.exe") $NodeExe -Force
    Copy-Item (Join-Path $tmp "$WinTarget\l1-wallet.exe") (Join-Path $InstallDir 'l1-wallet.exe') -Force -ErrorAction SilentlyContinue
    Remove-Item $tmp -Recurse -Force
    & $NodeExe --help | Select-Object -First 1 | ForEach-Object { Write-Note "node: $_" }
}

function Install-Genesis {
    Write-Step 'Fetching and verifying genesis'
    New-Item -ItemType Directory -Force -Path $DataDir, $LogDir, $NodeDataDir | Out-Null
    Invoke-WebRequest $GenesisUrl -OutFile $GenesisPath -UseBasicParsing
    $got = Get-Sha256 $GenesisPath
    if ($got -ne $GenesisSha256.ToLower()) {
        Remove-Item $GenesisPath -Force
        Die "genesis sha256 mismatch: got $got want $($GenesisSha256.ToLower()). A wrong genesis is a different chain."
    }
    Write-Note 'genesis verified against the published sha256'
}

function New-ValidatorKey {
    if (Test-Path $KeyPath) { Write-Note 'validator key already present; keeping it'; return }
    Write-Step 'Generating validator key'
    $pub = (& $NodeExe --generate-validator-key $KeyPath).Trim()
    Write-Host ''
    Write-Host "  Validator public key (register this via staking to enter the committee):" -ForegroundColor Yellow
    Write-Host "    $pub" -ForegroundColor Yellow
    Write-Host ''
    Set-Content -Path (Join-Path $DataDir 'validator.pub') -Value $pub
}

function Register-BootTask {
    Write-Step "Registering boot task '$TaskName'"
    $nodeArgs = @(
        '--genesis',       $GenesisPath,
        '--data-dir',      $NodeDataDir,
        '--validator-key', $KeyPath,
        '--rpc-addr',       $RpcAddr,
        '--p2p-port',       "$P2pPort",
        '--bootnode',       $Bootnode,
        '--prune-depth',    '3600',
        '--bootstrap-from', $Bootstrap,
        '--catch-up-from',  $Bootstrap
    ) -join ' '
    # A tiny launcher so logs are captured and the task line stays readable.
    $launcher = Join-Path $InstallDir 'run-node.cmd'
    @"
@echo off
rem Maya2C validator launcher. Started at boot by the "$TaskName" task.
"$NodeExe" $nodeArgs >> "$LogDir\node.log" 2>&1
"@ | Set-Content -Path $launcher -Encoding ASCII

    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    }
    $action  = New-ScheduledTaskAction -Execute $launcher
    $trigger = New-ScheduledTaskTrigger -AtStartup
    $princ   = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest
    # Restart if it ever exits; keep running indefinitely (a validator is a daemon).
    $set     = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
                   -StartWhenAvailable -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) `
                   -ExecutionTimeLimit ([TimeSpan]::Zero) -Priority $Priority
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger `
        -Principal $princ -Settings $set -Description $ServiceDesc | Out-Null
    Start-ScheduledTask -TaskName $TaskName
    Write-Note "started. Follow it with:  Get-Content '$LogDir\node.log' -Wait"
}

# --- Main --------------------------------------------------------------------
Assert-Admin
if ($Uninstall) { Remove-Install }

if (-not $GenesisUrl)    { Die 'join mode needs -GenesisUrl, where the seed operator published genesis.json' }
if (-not $GenesisSha256) { Die 'join mode needs -GenesisSha256, the hash the seed operator published with it' }
if (-not $Bootnode)      { Die 'join mode needs -Bootnode, the seed address /ip4/<ip>/tcp/31100' }

Show-Consent
if ($DryRun) { Write-Host 'DryRun: nothing was changed.' -ForegroundColor Green; exit 0 }

$tag = Get-ReleaseTag
Install-Node $tag
Install-Genesis
New-ValidatorKey
Register-BootTask

Write-Host ''
Write-Host 'Done. This machine is now a Maya2C testnet validator (single-operator).' -ForegroundColor Green
Write-Note "task:   Task Scheduler > '$TaskName'"
Write-Note "process: maya2c-node.exe"
Write-Note "logs:   $LogDir\node.log"
Write-Note "remove: .\join-validator.ps1 -Uninstall"
