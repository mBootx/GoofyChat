# Builds the Windows installer (NSIS, per-user) into dist-desktop\GoofyChat-setup.exe.
# Usage: powershell -ExecutionPolicy Bypass -File build\build-desktop.ps1 [-Server https://chat.example.org]
param([string]$Server)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$app = Join-Path $root 'app'

# Run a native command and fail on a non-zero exit code.
function Invoke-Step([string]$What, [scriptblock]$Cmd) {
    Write-Host "==> $What"
    & $Cmd
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)" }
}

# Tooling checks
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host 'cargo not found. Install Rust (MSVC toolchain) from https://rustup.rs'
    exit 1
}
$missing = $false
if (-not (Get-Command trunk -ErrorAction SilentlyContinue)) {
    Write-Host 'trunk not found. Install it with: cargo install trunk --locked'
    $missing = $true
}
cargo tauri --version *> $null
if ($LASTEXITCODE -ne 0) {
    Write-Host 'tauri-cli not found. Install it with: cargo install tauri-cli --version "^2" --locked'
    $missing = $true
}
if ($missing) { exit 1 }

if (-not ((rustup target list --installed) -contains 'wasm32-unknown-unknown')) {
    Invoke-Step 'rustup target add wasm32-unknown-unknown' { rustup target add wasm32-unknown-unknown }
}

# The UI reads the server address at compile time.
if ($Server) { $env:CHAT_SERVER = $Server }

Push-Location $app
try {
    Invoke-Step 'trunk build --release' { trunk build --release }
    Invoke-Step 'cargo tauri icon icon.svg' { cargo tauri icon icon.svg }
    Invoke-Step 'cargo tauri build --bundles nsis' { cargo tauri build --bundles nsis }
} finally {
    Pop-Location
}

# Locate the newest installer
$dirs = @()
if ($env:CARGO_TARGET_DIR) {
    $dirs += $env:CARGO_TARGET_DIR
} else {
    $dirs += (Join-Path $app 'src-tauri\target\release\bundle\nsis')
    $dirs += (Join-Path $root 'target\release\bundle\nsis')
}
$setup = $dirs | Where-Object { Test-Path $_ } |
    ForEach-Object { Get-ChildItem $_ -Recurse -Filter '*-setup.exe' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { throw "No *-setup.exe found under: $($dirs -join ', ')" }

$out = Join-Path $root 'dist-desktop'
New-Item -ItemType Directory -Force $out | Out-Null
$dest = Join-Path $out 'GoofyChat-setup.exe'
Copy-Item $setup.FullName $dest -Force
Write-Host ('Installer: {0} ({1:N1} MB)' -f $dest, ((Get-Item $dest).Length / 1MB))
