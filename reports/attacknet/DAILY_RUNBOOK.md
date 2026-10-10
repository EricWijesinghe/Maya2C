# Daily Attacknet Runbook

**Lab ID:** `maya2c-attacknet-lab-v1`
**Chain ID:** `maya2c-attacknet-lab`
**Schedule:** Daily at 02:00 local (configurable via Task Scheduler)
**Retention:** 28 days (4 weeks) of qualifying evidence

## Prerequisites (Checked Before Each Run)

- [ ] Host commit charge > 3.5 GB free (`$minFreeCommitGB`)
- [ ] D: drive > 20 GB free
- [ ] No critical Windows updates pending
- [ ] `master` branch accessible (GitHub reachable)
- [ ] Rust toolchain `nightly-2026-07-15` active
- [ ] `cargo xtask attacknet` builds cleanly

## Run Procedure

### 1. Prepare Clean Worktree (Isolated from Development)

```powershell
$repo = "D:\Maya2C"
$run = "D:\Maya2C-wt\attacknet-daily"
$publish = "D:\Maya2C-wt\attacknet-reports"

# Create/reset worktree at origin/master
if (-not (Test-Path $run)) {
    git -C $repo worktree add --detach $run origin/master
}
git -C $run fetch -q origin
git -C $run checkout -q --detach --force origin/master
$commit = (git -C $run rev-parse --short HEAD).Trim()
```

### 2. Resource Headroom Check

```powershell
function FreeGB { [math]::Round((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory / 1MB, 1) }
$freeGB = FreeGB
$minFree = 3.5
$abortFloor = 1

if ($freeGB -lt $minFree) {
    # Pause live testnet peers 7-8 to free commit charge
    $paused = Get-CimInstance Win32_Process -Filter "Name = 'maya2c-peer.exe'" |
        Where-Object { $_.CommandLine -match 'peer[78]\\data' }
    $paused | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
    Start-Sleep -Seconds 5
    $freeGB = FreeGB
}

if ($freeGB -lt $minFree) {
    # Skip run, publish skip report
    $body = "# Attacknet $(Get-Date -Format 'yyyy-MM-dd'): SKIPPED`n`nCommit free $freeGB GB, below $minFree GB needed. Master $commit.`n"
    Set-Content "$publish\$(Get-Date -Format 'yyyy-MM-dd')-skipped.md" $body
    git -C $publish add -A; git -C $publish commit -q -m "attacknet: $(Get-Date -Format 'yyyy-MM-dd') SKIPPED (master $commit)"; git -C $publish push -q origin attacknet-reports
    exit 0
}
```

### 3. Execute Attacknet

```powershell
$out = "$run\attacknet-daily.out"
$proc = Start-Process -FilePath 'cargo' -ArgumentList 'xtask', 'attacknet', '--rounds', '3' `
    -WorkingDirectory $run -RedirectStandardOutput $out -RedirectStandardError "$out.err" `
    -WindowStyle Hidden -PassThru

# Monitor commit charge during run
$aborted = $false
while (-not $proc.HasExited) {
    Start-Sleep -Seconds 10
    if ((FreeGB) -lt $abortFloor) {
        $aborted = $true
        # Kill only this run's processes
        Get-CimInstance Win32_Process |
            Where-Object { $_.ExecutablePath -like "$run\*" -or $_.CommandLine -like "*$run*" } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        break
    }
}
$proc.WaitForExit()
$code = if ($aborted) { 'aborted' } else { $proc.ExitCode }
```

### 4. Collect & Publish Report

```powershell
$report = Get-ChildItem "$run\reports\attacknet" -Filter '*.md' | Sort-Object LastWriteTime | Select-Object -Last 1
if ($report) {
    Copy-Item $report.FullName $publish -Force
    $file = Join-Path $publish $report.Name
} else {
    $file = Join-Path $publish "$(Get-Date -Format 'yyyy-MM-dd')-noreport.md"
    Set-Content $file "# Attacknet $(Get-Date -Format 'yyyy-MM-dd'): NO REPORT`n`nExit $code on master $commit`n"
}
```

### 5. Restore Paused Peers

```powershell
if ($paused) {
    & 'D:\Maya2C-peers\start-peers.ps1' -Count 8 | Out-Null
}
```

### 6. Commit & Push Reports

```powershell
git -C $publish add -A
git -C $publish commit -q -m "attacknet: $(Get-Date -Format 'yyyy-MM-dd') (master $commit)"
git -C $publish push -q origin attacknet-reports
```

## Success Criteria

- [ ] Report generated and published to `attacknet-reports` branch
- [ ] All 6 attacks pass (crash f, crash f+1, stolen key, garbage, RPC flood, long outage)
- [ ] No fork detected at any height
- [ ] All validators recover and rejoin
- [ ] Commit charge never dropped below 1 GB during run

## Failure Response

| Failure Type | Action |
|---|---|
| Build failure | Publish "NO REPORT", investigate next session |
| Attack failure (fork, stall, crash) | Preserve logs, triage per FAILURE_CATALOG.md, fix before next run |
| Resource exhaustion (aborted) | Publish "ABORTED", increase headroom or reduce validator count |
| Push failure | Retry once; if persistent, investigate GitHub auth |

## Evidence Collection

Each run produces:
- `reports/attacknet/<timestamp>.md` — Human-readable summary
- `reports/attacknet/runs/<date>/<scenario>/evidence.json` — Structured evidence (future)
- Raw logs in `target/attacknet/v*/node.log`

## Four-Week Completion Definition

**NOT COMPLETE** until:
- 28 consecutive daily runs with qualifying evidence exist
- "Qualifying" = report generated, attacks executed, verdict recorded
- Skipped days (resource constraints) documented but don't count toward 28
- Aborted days count as run days but flagged for investigation

## Manual Override

To run manually:
```powershell
cd D:\Maya2C
cargo xtask attacknet --rounds 3 --weighted
```

To run specific attack:
```powershell
# Not yet implemented - would need xtask sub-commands
```

## Contacts

- **Owner:** Eric (decision authority for fixes, resource allocation)
- **Automation:** This runbook (scheduled task)
- **Escalation:** If 3+ consecutive days fail/skip → manual intervention required