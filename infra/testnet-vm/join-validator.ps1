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
    Stop and remove the task, the binary and the chain data, then exit. The
    keys are kept (a lost validator key costs its seat); -Everything removes
    them too.

.PARAMETER Status
    Is the node running, in sync, and in the validator committee?

.PARAMETER Apply
    Make the wallet (asks for a new password) and write the validator
    application, which holds only public values, to the Desktop.

.PARAMETER Register
    After approval: register as a validator with this bond.

.PARAMETER Logs
    Follow the node's log.

.PARAMETER Update
    Install the newest release; keys and chain data are kept.

    The installer copies itself to C:\Program Files\Maya2C\maya2c-validator.ps1
    and adds Start-menu shortcuts (Maya2C > Status, Apply, Logs, Update,
    Uninstall), so none of these needs a path typed.

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
    [string]$Bootnode      = '/dns4/p2p.maya2c.dev/tcp/443/wss/p2p/12D3KooWJqH5AUQ1zPkMke18rcc8VHN2TWQ5XudGX3tP8zPLuyjB',
    # Snapshot/catch-up endpoint so a joining node loads a recent snapshot over
    # https (port 443, NAT-friendly) instead of replaying the chain from block 0.
    [string]$Bootstrap     = 'https://bootstrap.maya2c.dev/rpc',
    [string]$ReleaseTag,
    [string]$NodeSha256,
    [switch]$Silent,
    [switch]$IAmTheMachineOwner,
    [switch]$Uninstall,
    [switch]$Everything,
    [switch]$Status,
    [switch]$Apply,
    [ValidateRange(1, [long]::MaxValue)]
    [long]$Register,
    [switch]$Logs,
    [switch]$Update,
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
$WalletExe   = Join-Path $InstallDir 'l1-wallet.exe'
$WalletPath  = Join-Path $DataDir 'wallet.key'
$PubPath     = Join-Path $DataDir 'validator.pub'
$Settings    = Join-Path $DataDir 'settings.json'  # the ports this install chose
$SelfCopy    = Join-Path $InstallDir 'maya2c-validator.ps1'
$MenuDir     = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs\Maya2C'
$SelfUrl     = 'https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/infra/testnet-vm/join-validator.ps1'
$PublicRpc   = 'https://rpc.maya2c.dev/rpc'
$ApplyUrl    = 'https://github.com/EricWijesinghe/Maya2C/issues/new?template=validator-application.yml'
$SyncedWithin = 5   # blocks behind the network that still count as "in sync"
$RpcAddr     = "127.0.0.1:$RpcPort" # RPC is unauthenticated; bind loopback only.
# $P2pPort and $RpcPort come from params so a second node can coexist on one PC.

function Write-Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Write-Note($m) { Write-Host "    $m" -ForegroundColor DarkGray }
function Die($m)        { Write-Host "ERROR: $m" -ForegroundColor Red; exit 1 }

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole(
        [Security.Principal.WindowsBuiltinRole]::Administrator)
}

# Re-launch this script in an Administrator window with the same switches,
# so a Start-menu shortcut or a double-click works without "Run as admin".
function Assert-Admin {
    if (Test-Admin) { return }
    $fwd = @('-NoProfile', '-NoExit', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    foreach ($kv in $PSBoundParameters.GetEnumerator()) {
        if ($kv.Value -is [switch]) { if ($kv.Value) { $fwd += "-$($kv.Key)" } }
        else { $fwd += "-$($kv.Key)"; $fwd += "`"$($kv.Value)`"" }
    }
    Write-Host 'Windows will ask for permission. Click Yes.' -ForegroundColor Cyan
    Start-Process powershell.exe -Verb RunAs -ArgumentList $fwd
    exit 0
}

function Remove-Install {
    Write-Step 'Uninstalling Maya2C validator'
    if (Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue) {
        Stop-ScheduledTask    -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Note "removed scheduled task '$TaskName'"
    }
    # By the path this install put it at, never by image name: a second node
    # on the same PC (the seed operator's own) must survive an uninstall.
    Get-Process maya2c-node -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $NodeExe } | Stop-Process -Force
    $doomed = @($InstallDir, $MenuDir, $NodeDataDir, $LogDir)
    if ($Everything) { $doomed += $DataDir }
    foreach ($d in $doomed) {
        if (Test-Path $d) { Remove-Item $d -Recurse -Force; Write-Note "removed $d" }
    }
    if (-not $Everything -and (Test-Path $DataDir)) {
        Write-Note "your keys are kept in $DataDir (delete it only if you will never validate again)"
    }
    Write-Host 'Uninstalled.' -ForegroundColor Green
    exit 0
}

# Keys readable by SYSTEM and Administrators only. ProgramData otherwise
# lets every local user read what is under it.
function Protect-Secret($path) {
    & icacls $path /inheritance:r /grant:r '*S-1-5-18:F' '*S-1-5-32-544:F' | Out-Null
    if ($LASTEXITCODE -ne 0) { Die "could not restrict access to $path (icacls exit $LASTEXITCODE)" }
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
    Write-Host '      Start menu > Maya2C > Uninstall Maya2C' -ForegroundColor White
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
    # Use Windows' bundled bsdtar by full path, not whatever `tar` is first on
    # PATH: a GNU tar (e.g. from Git) reads the "D:\..." temp dir as a remote
    # host "D:" and fails with "Cannot connect to D:". bsdtar handles drive
    # paths. System32\tar.exe is bsdtar on Windows 10+.
    $tarExe = Join-Path $env:SystemRoot 'System32\tar.exe'
    if (-not (Test-Path $tarExe)) { $tarExe = 'tar' }
    & $tarExe -xzf $arcPath -C $tmp
    if ($LASTEXITCODE -ne 0) { Die "extracting $archive failed (tar exit $LASTEXITCODE)" }
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
    # An older install left the key readable by every local user; fix that too.
    if (Test-Path $KeyPath) { Protect-Secret $KeyPath; Write-Note 'validator key already present; keeping it'; return }
    Write-Step 'Generating validator key'
    $pub = (& $NodeExe --generate-validator-key $KeyPath).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $pub) { Die 'making the validator key failed' }
    Protect-Secret $KeyPath
    Set-Content -Path $PubPath -Value $pub
    Write-Note 'made a new validator key. It never leaves this computer.'
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
    @{ RpcPort = $RpcPort; P2pPort = $P2pPort } | ConvertTo-Json | Set-Content $Settings
    Write-Note 'started'
}

# The installer stays on the machine as the helper behind the Start-menu
# shortcuts. Under `irm | iex` there is no file to copy, so fetch it.
function Install-Helper {
    if ($PSCommandPath -and (Test-Path $PSCommandPath)) {
        if ($PSCommandPath -ne $SelfCopy) { Copy-Item $PSCommandPath $SelfCopy -Force }
    } else {
        Invoke-WebRequest $SelfUrl -OutFile $SelfCopy -UseBasicParsing
    }
    New-Item -ItemType Directory -Force -Path $MenuDir | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $items = [ordered]@{
        'Maya2C Status'             = '-Status'
        'Maya2C Become a validator' = '-Apply'
        'Maya2C Logs'               = '-Logs'
        'Maya2C Update'             = '-Update'
        'Uninstall Maya2C'          = '-Uninstall'
    }
    foreach ($name in $items.Keys) {
        $lnk = $shell.CreateShortcut((Join-Path $MenuDir "$name.lnk"))
        $lnk.TargetPath = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
        $lnk.Arguments  = "-NoProfile -NoExit -ExecutionPolicy Bypass -File `"$SelfCopy`" $($items[$name])"
        $lnk.Save()
    }
    Write-Note "Start menu > Maya2C: Status, Become a validator, Logs, Update, Uninstall"
}

function Get-RpcPort {
    if (Test-Path $Settings) { return (Get-Content $Settings -Raw | ConvertFrom-Json).RpcPort }
    return $RpcPort
}

function Invoke-Rpc($url, $method) {
    $body = @{ jsonrpc = '2.0'; id = 1; method = $method; params = @() } | ConvertTo-Json
    try { (Invoke-RestMethod $url -Method Post -Body $body -ContentType 'application/json' -TimeoutSec 5).result }
    catch { $null }
}

function Show-Status {
    Write-Host ''
    Write-Host '  Maya2C node status' -ForegroundColor White
    # This install's task, not any maya2c-node.exe: another node on the PC
    # would otherwise answer for this one.
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if (-not $task -or $task.State -ne 'Running') {
        Write-Host '  X the node is not running' -ForegroundColor Red
        Write-Note "restart the PC, or run in Administrator PowerShell: Start-ScheduledTask '$TaskName'"
        return
    }
    Write-Host '  OK the node is running' -ForegroundColor Green
    $local = "http://127.0.0.1:$(Get-RpcPort)"
    $mine = Invoke-Rpc $local 'get_tip_height'
    $net  = Invoke-Rpc $Bootstrap 'get_tip_height'
    if ($null -eq $mine) { Write-Note 'downloading the chain - the first start can take up to an hour' }
    elseif ($null -ne $net -and ($net - $mine) -le $SyncedWithin) {
        Write-Host "  OK in sync: block $mine (network: $net)" -ForegroundColor Green
    } else {
        Write-Note "catching up: block $mine of $net"
        Write-Note 'The first start can take up to an hour. Nothing to do but wait.'
    }
    $bft = Invoke-Rpc $local 'get_bft_status'
    if ($bft -and $null -ne $bft.validator) {
        Write-Host '  OK validating: your node is in the committee and voting' -ForegroundColor Green
    } else { Write-Note 'following the chain (not a validator yet: Start menu > Maya2C Become a validator)' }
    Write-Note "files: $DataDir    log: $LogDir\node.log"
}

function Invoke-Wallet {
    $env:L1_WALLET_KEYSTORE = $WalletPath
    & $WalletExe --rpc-url $PublicRpc @args
    if ($LASTEXITCODE -ne 0) { Die "l1-wallet $($args[0]) failed (exit $LASTEXITCODE)" }
}

function Invoke-Apply {
    if (-not (Test-Path $PubPath)) { Die 'no validator key yet: run the installer first' }
    if (-not (Test-Path $WalletPath)) {
        Write-Step 'Making your wallet. Choose a password and write it down: it cannot be recovered.'
        Invoke-Wallet generate
        Protect-Secret $WalletPath
    }
    $address = (Invoke-Wallet address | Select-Object -Last 1).Trim()
    $file = Join-Path ([Environment]::GetFolderPath('Desktop')) 'maya2c-application.txt'
    @('Maya2C validator application', '', 'Wallet address:', $address, '',
      'Validator public key:', (Get-Content $PubPath -Raw).Trim()) | Set-Content $file
    Write-Host ''
    Write-Host "  Your application is on the Desktop: $file" -ForegroundColor Green
    Write-Note 'It holds only PUBLIC information. Send it to Eric any way you like,'
    Write-Note "or paste the two values into the form: $ApplyUrl"
    Write-Note 'When Eric says you are approved and funded, run in this window:'
    Write-Note "  & '$SelfCopy' -Register <the amount Eric gives you>"
    Start-Process notepad.exe $file
}

function Invoke-Register {
    if (-not (Test-Path $WalletPath)) { Die 'no wallet yet: Start menu > Maya2C Become a validator' }
    Write-Step 'Registering your validator (your wallet password is needed)'
    Invoke-Wallet register-validator --validator-key $KeyPath --bond $Register
    Write-Host '  Registered. You join the committee at the next epoch (within about an hour).' -ForegroundColor Green
}

# --- Main --------------------------------------------------------------------
# Status and Logs only read, so they need no Administrator window.
if ($Status) { Show-Status; exit 0 }
if ($Logs)   { Get-Content (Join-Path $LogDir 'node.log') -Tail 50 -Wait; exit 0 }
Assert-Admin
if ($Uninstall) { Remove-Install }
if ($Apply)     { Invoke-Apply; exit 0 }
if ($Register)  { Invoke-Register; exit 0 }
if ($Update) {
    # The newest installer, not this copy of it: an update may fix the installer.
    Invoke-WebRequest $SelfUrl -OutFile $SelfCopy -UseBasicParsing
    & $SelfCopy -Silent -IAmTheMachineOwner -P2pPort $P2pPort -RpcPort (Get-RpcPort)
    exit $LASTEXITCODE
}

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
Install-Helper

Write-Host ''
Write-Host 'Done. Thank you for helping!' -ForegroundColor Green
Write-Note 'The node runs in the background and starts by itself with Windows.'
Write-Note 'Start menu > Maya2C Status            is it running and in sync?'
Write-Note 'Start menu > Maya2C Become a validator  the next step'
Write-Note "Files: $DataDir   Log: $LogDir\node.log"
