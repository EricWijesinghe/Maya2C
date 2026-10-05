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
# Seven validators at about 150 MB each, plus an incremental build.
$minFreeCommitGB = 3

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

$freeGB = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory / 1MB, 1)
$date = Get-Date -Format 'yyyy-MM-dd'
if ($freeGB -lt $minFreeCommitGB) {
    $body = "# Attacknet $date: SKIPPED`n`nCommit free $freeGB GB, below the $minFreeCommitGB GB needed for seven validators. Master $commit.`n"
    $file = Join-Path $publish "$date-skipped.md"
    Set-Content -Path $file -Value $body
    Say "skipped: $freeGB GB commit free"
} else {
    Push-Location $run
    try {
        cargo xtask attacknet --rounds 3 *> (Join-Path $run 'attacknet-daily.out')
        $code = $LASTEXITCODE
    } finally { Pop-Location }
    $report = Get-ChildItem (Join-Path $run 'reports\attacknet') -Filter '*.md' |
        Sort-Object LastWriteTime | Select-Object -Last 1
    if (-not $report) { Say "no report written (exit $code)"; exit 1 }
    Copy-Item $report.FullName $publish -Force
    $file = Join-Path $publish $report.Name
    Say "ran: exit $code, report $($report.Name), master $commit"
}

git -C $publish add -A
git -C $publish commit -q -m "attacknet: $date (master $commit)"
git -C $publish push -q origin attacknet-reports
Say "published $(Split-Path $file -Leaf)"
