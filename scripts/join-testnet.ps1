# One line from a fresh Windows PC to a maya-testnet-1 node. Paste into
# PowerShell (Start menu > type "PowerShell" > Enter):
#
#   irm https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.ps1 | iex
#
# This file is only the doorway. `irm | iex` cannot pass parameters or run
# elevated, so it downloads the real installer (infra/testnet-vm/join-validator.ps1)
# and starts it in an Administrator window; Windows asks "Do you want to allow
# this app to make changes?" once, and the helper answers Yes. The installer
# then shows exactly what it will do and asks before doing it.
$ErrorActionPreference = 'Stop'
$url  = 'https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/infra/testnet-vm/join-validator.ps1'
$dest = Join-Path $env:TEMP 'maya2c-join-validator.ps1'

Write-Host 'Maya2C: downloading the installer...' -ForegroundColor Cyan
Invoke-WebRequest $url -OutFile $dest -UseBasicParsing
Write-Host 'Maya2C: Windows will now ask for permission. Click Yes.' -ForegroundColor Cyan
# -NoExit keeps the window open so the result stays readable.
Start-Process powershell.exe -Verb RunAs -ArgumentList @(
    '-NoProfile', '-NoExit', '-ExecutionPolicy', 'Bypass', '-File', "`"$dest`""
)
