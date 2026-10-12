<#
.SYNOPSIS
    Keep the foundation validators running on one Windows PC (ADR-047, gate 5).
.DESCRIPTION
    All four foundation validators run on Eric's PC. One machine means one
    set of failure modes, and this script closes the three that are the
    host's fault rather than the chain's:

      1. The watchdog only ran while Eric was logged in (LogonType
         Interactive). After an unattended reboot nothing started until
         someone logged in, so a 3 a.m. update restart would halt the chain
         until morning. -Apply re-registers it to run at boot and every five
         minutes, logged in or not (S4U: no password is stored).
      2. Windows Update restarted the PC on 2026-10-04 and halted
         maya-testnet-1 for 2 h 45 min. -Apply sets updates to download and
         then wait to be installed by hand (AUOptions 3), and never to
         restart while someone is logged in.
      3. Sleep on mains power stops every validator at once. -Apply turns
         AC sleep and hibernate off. Battery settings are left alone.

    It changes nothing it does not print, and -Check (the default) changes
    nothing at all.
.PARAMETER Check
    Report each item as OK or NEEDS FIX, and query the four validators. Read-only.
.PARAMETER Apply
    Fix every NEEDS FIX item. Needs an Administrator PowerShell.
.PARAMETER SelfTest
    Prove the watchdog runs with nobody logged in. Registers a one-shot task
    with the same principal, runs it, reads the Windows session it ran in,
    and deletes it. Session 0 is the non-interactive session that exists
    before anyone logs in. Needs Administrator.
.EXAMPLE
    powershell -ExecutionPolicy Bypass -File infra\foundation-pc\harden-host.ps1
.EXAMPLE
    # In an Administrator PowerShell:
    powershell -ExecutionPolicy Bypass -File infra\foundation-pc\harden-host.ps1 -Apply
    powershell -ExecutionPolicy Bypass -File infra\foundation-pc\harden-host.ps1 -SelfTest
#>
[CmdletBinding(DefaultParameterSetName = 'Check')]
param(
    [Parameter(ParameterSetName = 'Check')][switch]$Check,
    [Parameter(ParameterSetName = 'Apply')][switch]$Apply,
    [Parameter(ParameterSetName = 'SelfTest')][switch]$SelfTest
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WatchdogTask   = 'Maya2C testnet watchdog'
$WatchdogPeriod = New-TimeSpan -Minutes 5
$AuKey          = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU'
# 3 = download automatically, install only when told to. Nothing installs, so
# nothing needs a restart.
$AuNotifyInstall = 3
$SelfTestTask   = 'Maya2C session-0 selftest'
$SelfTestWaitS  = 30
# The seed and v2..v4, as start-testnet.ps1 and start-validators.ps1 bind them.
$ValidatorRpc   = [ordered]@{ seed = 8545; v2 = 18602; v3 = 18603; v4 = 18604 }

function Test-Admin {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    (New-Object Security.Principal.WindowsPrincipal $id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Write-Item([string]$name, [bool]$ok, [string]$detail) {
    $tag = if ($ok) { 'OK       ' } else { 'NEEDS FIX' }
    $colour = if ($ok) { 'Green' } else { 'Yellow' }
    Write-Host ("[{0}] {1}: {2}" -f $tag, $name, $detail) -ForegroundColor $colour
}

function Get-AcPowerIndex([string]$setting) {
    $out = powercfg /query SCHEME_CURRENT SUB_SLEEP $setting
    $line = $out | Select-String 'Current AC Power Setting Index' | Select-Object -First 1
    if (-not $line) { throw "powercfg did not report $setting" }
    [Convert]::ToInt32(($line.ToString() -split ':')[1].Trim(), 16)
}

function Get-HostState {
    $task = Get-ScheduledTask -TaskName $WatchdogTask -ErrorAction SilentlyContinue
    $au = Get-ItemProperty $AuKey -ErrorAction SilentlyContinue
    $prop = { param($o, $n) if ($o -and ($o.PSObject.Properties.Name -contains $n)) { $o.$n } else { $null } }
    [pscustomobject]@{
        TaskExists   = [bool]$task
        LogonType    = if ($task) { [string]$task.Principal.LogonType } else { '' }
        BootTrigger  = if ($task) { [bool]($task.Triggers | Where-Object { $_.CimClass.CimClassName -eq 'MSFT_TaskBootTrigger' }) } else { $false }
        AuOptions    = & $prop $au 'AUOptions'
        NoAutoReboot = & $prop $au 'NoAutoRebootWithLoggedOnUsers'
        StandbyAc    = Get-AcPowerIndex 'STANDBYIDLE'
        HibernateAc  = Get-AcPowerIndex 'HIBERNATEIDLE'
    }
}

function Show-Validators {
    $body = '{"jsonrpc":"2.0","id":1,"method":"get_bft_status","params":[]}'
    foreach ($name in $ValidatorRpc.Keys) {
        $uri = "http://127.0.0.1:$($ValidatorRpc[$name])"
        try {
            $r = (Invoke-RestMethod -Uri $uri -Method Post -ContentType 'application/json' -Body $body -TimeoutSec 5).result
            $voting = ($null -ne $r.validator) -and (-not $r.follower)
            Write-Item "validator $name" $voting ("epoch {0}, committed round {1}, seat {2}, committee {3}" -f $r.epoch, $r.committed_round, $r.validator, $r.committee)
        } catch {
            Write-Item "validator $name" $false "no answer on $uri"
        }
    }
}

function Show-Insider {
    $flight = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\WindowsSelfHost\Applicability' -ErrorAction SilentlyContinue
    $branch = if ($flight -and ($flight.PSObject.Properties.Name -contains 'BranchName')) { $flight.BranchName } else { '' }
    # Insider builds can schedule their own restarts; no policy here overrides
    # that, so it is reported and left to Eric (Settings > Windows Update >
    # Windows Insider Program > Stop getting preview builds).
    Write-Item 'Windows Insider builds' (-not $branch) $(if ($branch) { "enrolled ($branch): leave the programme before mainnet" } else { 'not enrolled' })
}

function Invoke-Check {
    $s = Get-HostState
    Write-Item 'watchdog task' ($s.TaskExists -and $s.BootTrigger -and $s.LogonType -in 'S4U', 'Password') ("exists={0}, boot trigger={1}, logon type={2}" -f $s.TaskExists, $s.BootTrigger, $s.LogonType)
    Write-Item 'updates wait to be installed' ($s.AuOptions -eq $AuNotifyInstall) "AUOptions=$($s.AuOptions)"
    Write-Item 'no restart while logged on' ($s.NoAutoReboot -eq 1) "NoAutoRebootWithLoggedOnUsers=$($s.NoAutoReboot)"
    Write-Item 'no sleep on mains power' ($s.StandbyAc -eq 0 -and $s.HibernateAc -eq 0) ("standby after {0} s, hibernate after {1} s" -f $s.StandbyAc, $s.HibernateAc)
    Show-Insider
    Show-Validators
}

function Set-WatchdogPrincipal {
    $task = Get-ScheduledTask -TaskName $WatchdogTask -ErrorAction SilentlyContinue
    if (-not $task) { throw "no '$WatchdogTask' task: run D:\Maya2C-testnet\start-testnet.ps1 setup first" }
    $user = $task.Principal.UserId
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType S4U -RunLevel Limited
    # Keep the task's own five-minute repeating trigger (a new -Once trigger's
    # repetition length differs between Windows builds) and add boot.
    $kept = @($task.Triggers | Where-Object { $_.CimClass.CimClassName -ne 'MSFT_TaskBootTrigger' })
    $boot = New-ScheduledTaskTrigger -AtStartup
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -MultipleInstances IgnoreNew -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
    Set-ScheduledTask -TaskName $WatchdogTask -Principal $principal -Trigger (@($boot) + $kept) -Settings $settings | Out-Null
    Write-Host "watchdog: runs as $user at boot and every $($WatchdogPeriod.TotalMinutes) min, logged on or not"
}

function Invoke-Apply {
    if (-not (Test-Admin)) { throw 'Apply needs an Administrator PowerShell (Start > type PowerShell > Run as administrator)' }
    Set-WatchdogPrincipal
    New-Item -Path $AuKey -Force | Out-Null
    Set-ItemProperty -Path $AuKey -Name 'NoAutoUpdate' -Value 0 -Type DWord
    Set-ItemProperty -Path $AuKey -Name 'AUOptions' -Value $AuNotifyInstall -Type DWord
    Set-ItemProperty -Path $AuKey -Name 'NoAutoRebootWithLoggedOnUsers' -Value 1 -Type DWord
    Write-Host 'updates: download automatically, install only by hand; no restart while logged on'
    powercfg /change standby-timeout-ac 0
    powercfg /change hibernate-timeout-ac 0
    Write-Host 'power: no sleep or hibernate on mains power'
    Write-Host ''
    Invoke-Check
}

function Invoke-SelfTest {
    if (-not (Test-Admin)) { throw 'SelfTest needs an Administrator PowerShell' }
    $user = (Get-ScheduledTask -TaskName $WatchdogTask).Principal.UserId
    $out = Join-Path $env:TEMP 'maya2c-session-selftest.txt'
    Remove-Item $out -ErrorAction SilentlyContinue
    $cmd = "[Diagnostics.Process]::GetCurrentProcess().SessionId | Set-Content -Path '$out'"
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -Command `"$cmd`""
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType S4U -RunLevel Limited
    Register-ScheduledTask -TaskName $SelfTestTask -Action $action -Principal $principal -Force | Out-Null
    try {
        Start-ScheduledTask -TaskName $SelfTestTask
        $deadline = (Get-Date).AddSeconds($SelfTestWaitS)
        while (-not (Test-Path $out) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 500 }
        if (-not (Test-Path $out)) { throw "the self-test task wrote nothing within $SelfTestWaitS s" }
        $session = [int](Get-Content $out)
        Write-Item 'runs with nobody logged in' ($session -eq 0) "a task with the watchdog's principal ran in Windows session $session (0 = no logon needed)"
    } finally {
        Unregister-ScheduledTask -TaskName $SelfTestTask -Confirm:$false
        Remove-Item $out -ErrorAction SilentlyContinue
    }
}

switch ($PSCmdlet.ParameterSetName) {
    'Apply'    { Invoke-Apply }
    'SelfTest' { Invoke-SelfTest }
    default    { Invoke-Check }
}
