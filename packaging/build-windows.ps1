# Build ArbolView-<version>-windows-<arch>.zip in app\dist\ — run on Windows with Rust (MSVC).
#   powershell -File packaging\build-windows.ps1                 # x64 (Intel/AMD)
#   powershell -File packaging\build-windows.ps1 -Arch arm64     # ARM (Snapdragon, Surface Pro X...)
# The zip holds a single ArbolView.exe (icon included) that needs no installation.
param([ValidateSet("x64", "arm64")][string]$Arch = "x64")
$ErrorActionPreference = "Stop"
$AppDir = Split-Path -Parent $PSScriptRoot
Set-Location $AppDir
$Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$Target = if ($Arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }

# keep the build folder's location out of the program
$env:CARGO_ENCODED_RUSTFLAGS = "--remap-path-prefix=$AppDir=arbolview"
rustup target add $Target
cargo build --release --target $Target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force -Path dist | Out-Null
$Stage = Join-Path $env:TEMP "arbolview-win-$Arch"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $Stage | Out-Null
Copy-Item "target\$Target\release\arbolview.exe" (Join-Path $Stage "ArbolView.exe")
$Zip = "dist\ArbolView-$Version-windows-$Arch.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Zip
Write-Output $Zip
