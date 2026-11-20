# Build ArbolView-<version>-windows-x64.zip in app\dist\ — run on Windows with Rust (MSVC).
# The zip holds a single arbolview.exe (icon included) that needs no installation.
$ErrorActionPreference = "Stop"
$AppDir = Split-Path -Parent $PSScriptRoot
Set-Location $AppDir
$Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value

# keep the build folder's location out of the program
$env:CARGO_ENCODED_RUSTFLAGS = "--remap-path-prefix=$AppDir=arbolview"
cargo build --release
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

New-Item -ItemType Directory -Force -Path dist | Out-Null
$Stage = Join-Path $env:TEMP "arbolview-win"
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $Stage | Out-Null
Copy-Item target\release\arbolview.exe (Join-Path $Stage "ArbolView.exe")
$Zip = "dist\ArbolView-$Version-windows-x64.zip"
Remove-Item -Force $Zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $Stage "*") -DestinationPath $Zip
Write-Output $Zip
