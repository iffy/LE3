# Package a portable Windows release executable.
# Usage: pwsh scripts/package-windows.ps1
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$source = Join-Path $root "target\release\bearcad.exe"
$dest = Join-Path $root "bearcad.exe"

if (-not (Test-Path $source)) {
    throw "Release binary not found: $source (run 'cargo build --release' first)"
}

Copy-Item -Force $source $dest
Write-Host "Created bearcad.exe"