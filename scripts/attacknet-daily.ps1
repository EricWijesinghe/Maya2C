# Daily self-run attacknet (ADR-042, gate 6): every attack, three rounds,
# against the current master, for four weeks.
#
# Run by the "Maya2C attacknet daily" scheduled task. Each run:
#   1. resets a dedicated worktree to origin/master (never a working copy
#      anyone edits);
#   2. skips, and says so, when the machine lacks the commit headroom for
#      seven validators: a skipped day is recorded, never silently missing;
#   3. runs `cargo xtask attacknet --rounds 3`;
#   4. commits the dated report to the `attacknet-reports` branch and pushes
#      it, so the site's break-it page links real results.
#
# Pass or fail, the report is published: a failing day is the point of an
# attacknet, not something to hide.
$ErrorActionPreference = 'Stop'
$repo    = 'D:\Maya2C'
$run     = 'D:\Maya2C-wt\attacknet-daily'
$publish = 'D:\Maya2C-wt\attacknet-reports'
$log     = 'D:\Maya2C-ops\attacknet-daily.log'
# Seven validators at about 150 MB each, plus an incremental build, and a
# margin on top: the live testnet's validators (maya2c-validator.exe and the
# seed) share this PC, and with n = 4, two of them failing an allocation
# halts the chain. The attacknet never touches those processes; this keeps it
# from starving them.
$minFreeCommitGB = 3.5
# During the run: below this, the run kills its own processes and stops.
$abortFloorGB = 1

New-Item -ItemType Directory -Force -Path (Split-Path $log) | Out-Null
function Say([string]$line) { Add-Content -Path $log -Value "$(Get-Date -Format s) $line" }

if (-not (Test-Path $run)) {
    git -C $repo worktree add --detach $run origin/master | Out-Null
}
if (-not (Test-Path $publish)) {
    git -C $repo fetch origin attacknet-reports 2>$null
    if ($LASTEXITCODE -eq 0) {
        git -C $repo worktree add $publish attacknet-reports | Out-Null
    } else {
        git -C $repo worktree add --orphan -b attacknet-reports $publish | Out-Null
    }
}

git -C $run fetch -q origin
git -C $run checkout -q --detach --force origin/master
$commit = (git -C $run rev-parse --short HEAD).Trim()

function FreeGB { [math]::Round((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory / 1MB, 1) }
$date = Get-Date -Format 'yyyy-MM-dd'
# Short of headroom: pause the project peers 11-12 for the run. They are
# test load, the attacknet is gate evidence. They are restarted afterwards,
# whatever the run did.
$paused = @()
if ((FreeGB) -lt $minFreeCommitGB) {
    $paused = Get-CimInstance Win32_Process -Filter "Name = 'maya2c-peer.exe'" |
        Where-Object { $_.CommandLine -match 'peer1[1-2]\\data' }
    $paused | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    if ($paused) { Start-Sleep -Seconds 5; Say "paused $($paused.Count) peers for the run" }
}
$freeGB = FreeGB
if ($freeGB -lt $minFreeCommitGB) {
    $body = "# Attacknet ${date}: SKIPPED`n`nCommit free $freeGB GB, below the $minFreeCommitGB GB needed for seven validators. Master $commit.`n"
    $file = Join-Path $publish "$date-skipped.md"
    Set-Content -Path $file -Value $body
    Say "skipped: $freeGB GB commit free"
} else {
    # Run in the background and watch commit headroom: if it falls below the
    # floor, kill this run's own processes (everything started from the
    # runner worktree) before the live validators start failing allocations.
    $out = Join-Path $run 'attacknet-daily.out'
    $proc = Start-Process -FilePath 'cargo' -ArgumentList 'xtask', 'attacknet', '--rounds', '3' `
        -WorkingDirectory $run -RedirectStandardOutput $out -RedirectStandardError "$out.err" `
        -WindowStyle Hidden -PassThru
    $aborted = $false
    while (-not $proc.HasExited) {
        Start-Sleep -Seconds 10
        if ((FreeGB) -lt $abortFloorGB) {
            $aborted = $true
            Get-CimInstance Win32_Process |
                Where-Object { $_.ExecutablePath -like "$run\*" -or $_.CommandLine -like "*$run*" } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            Say "aborted: commit free fell below $abortFloorGB GB during the run"
            break
        }
    }
    $proc.WaitForExit()
    $code = if ($aborted) { 'aborted' } else { $proc.ExitCode }
    $report = Get-ChildItem (Join-Path $run 'reports\attacknet') -Filter '*.md' |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if ($report) {
        Copy-Item $report.FullName $publish -Force
        $file = Join-Path $publish $report.Name
        Say "ran: exit $code, report $($report.Name), master $commit"
    } else {
        # The harness died before writing one (a build failure, say): publish
        # that, rather than nothing.
        $file = Join-Path $publish "$date-noreport.md"
        Set-Content -Path $file -Value "# Attacknet ${date}: NO REPORT`n`nExit $code on master $commit; see attacknet-daily.out on the runner.`n"
        Say "no report written (exit $code)"
    }
}

if ($paused) {
    & 'D:\Maya2C-peers\start-peers.ps1' -Count 12 | Out-Null
    Say "restarted the paused peers"
}

git -C $publish add -A
git -C $publish commit -q -m "attacknet: $date (master $commit)"
git -C $publish push -q origin attacknet-reports
Say "published $(Split-Path $file -Leaf)"
