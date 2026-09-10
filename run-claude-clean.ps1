$ErrorActionPreference = "Continue"

# 1. Setup Drive D Environment
$env:HF_HOME          = "D:\Caches\hf_cache"
$env:PIP_CACHE_DIR    = "D:\Caches\pip_cache"
$env:npm_config_cache = "D:\Caches\npm_cache"
$env:UV_CACHE_DIR     = "D:\Caches\uv_cache"

Set-Location "D:\Maya2C"
Write-Host "Launching Claude Code... (Auto-cleanup will run when you exit)" -ForegroundColor Green

try {
    # 2. Run Claude Code
    claude
} finally {
    # 3. Post-Run Automatic Cleanup
    Write-Host "
==================================================" -ForegroundColor Magenta
    Write-Host " CLAUDE SESSION ENDED. RUNNING DISK CLEANUP...    " -ForegroundColor Magenta
    Write-Host "==================================================" -ForegroundColor Magenta
    
    if (Test-Path "Cargo.toml") {
        Write-Host "Sweeping Rust build artifacts older than 1 day..." -ForegroundColor Cyan
        cargo sweep --time 1
        
        # Optional: Uncomment the line below if you want a TOTAL wipe every time instead of a smart sweep
        # cargo clean
    }

    Write-Host "Cleanup complete. Hard drive space recovered!" -ForegroundColor Green
    Start-Sleep -Seconds 2
}
