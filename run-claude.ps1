# Dedicated Claude Code Launcher for D:\Maya2C
$env:HF_HOME          = "D:\Caches\hf_cache"
$env:PIP_CACHE_DIR    = "D:\Caches\pip_cache"
$env:npm_config_cache = "D:\Caches\npm_cache"
$env:UV_CACHE_DIR     = "D:\Caches\uv_cache"

Set-Location "D:\Maya2C"
Write-Host "Launching Claude Code CLI in D:\Maya2C with D: Drive caches..." -ForegroundColor Green
claude
