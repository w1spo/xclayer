$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
$env:CARGO_TERM_COLOR = 'never'
$toolchain = 'stable-x86_64-pc-windows-msvc'
$log = Join-Path $PSScriptRoot 'build_patch16.log'
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { throw 'Rustup not found. Install Rust using https://rustup.rs/ and Visual Studio C++ Build Tools with Windows SDK.' }
& rustup toolchain install $toolchain --profile minimal
if ($LASTEXITCODE -ne 0) { throw 'Rust toolchain installation failed.' }
# cmd redirects both native output streams without PowerShell treating stderr as an exception.
& cmd.exe /d /c "cargo +$toolchain build --release --workspace --locked > build_patch16.log 2>&1"
$code = $LASTEXITCODE
Get-Content -LiteralPath $log
if ($code -ne 0) { throw "Build failed (exit $code). Log: $log" }
$exe = Join-Path $PSScriptRoot 'target\release\xcl.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Build finished but executable not found at $exe" }
Write-Host "Build succeeded: $exe" -ForegroundColor Green
Write-Host "Log: $log"
