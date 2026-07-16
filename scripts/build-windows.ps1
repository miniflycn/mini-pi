param(
    [string]$Version = "0.1.0"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$root = Split-Path -Parent $PSScriptRoot
$package = Join-Path $root "target\package\mini-pi"
$tools = Join-Path $root "tools"
$wixOut = Join-Path $root "target\wix"
$msiOut = Join-Path $root "target\mini-pi-$Version-x64.msi"

New-Item -ItemType Directory -Force -Path $tools | Out-Null
New-Item -ItemType Directory -Force -Path $wixOut | Out-Null

Write-Host "Building release binary..."
cargo build --release

Write-Host "Staging installer files..."
if (Test-Path $package) {
    Remove-Item -Recurse -Force $package
}
New-Item -ItemType Directory -Force -Path $package | Out-Null

Copy-Item (Join-Path $root "target\release\mini-pi.exe") $package
Copy-Item -Recurse (Join-Path $root "assets") $package
Copy-Item -Recurse (Join-Path $root "scripts\installer") $package

$bridgeStage = Join-Path $package "pi-bridge"
New-Item -ItemType Directory -Force -Path $bridgeStage | Out-Null
Copy-Item (Join-Path $root "pi-bridge\package.json") $bridgeStage
Copy-Item (Join-Path $root "pi-bridge\tsconfig.json") $bridgeStage
Copy-Item (Join-Path $root "pi-bridge\bun.lock") $bridgeStage
Copy-Item -Recurse (Join-Path $root "pi-bridge\src") $bridgeStage

$bunVersion = "1.2.22"
$bunZip = Join-Path $tools "bun-windows-x64.zip"
$bunExtract = Join-Path $tools "bun-$bunVersion-windows-x64"
if (-not (Test-Path $bunZip)) {
    Write-Host "Downloading Bun v$bunVersion ..."
    Invoke-WebRequest -Uri "https://github.com/oven-sh/bun/releases/download/bun-v$bunVersion/bun-windows-x64.zip" -OutFile $bunZip
}
if (-not (Test-Path $bunExtract)) {
    New-Item -ItemType Directory -Force -Path $bunExtract | Out-Null
    Expand-Archive -Path $bunZip -DestinationPath $bunExtract -Force
}
$bunExe = Join-Path $bunExtract "bun-windows-x64\bun.exe"

Write-Host "Preparing pi-bridge bundle..."
Push-Location $bridgeStage

$miniPiDir = Join-Path $env:USERPROFILE ".mini-pi"
$bunCacheDir = Join-Path $miniPiDir "bun-cache"
New-Item -ItemType Directory -Force -Path (Join-Path $bunCacheDir "install-cache") | Out-Null

$env:BUN_INSTALL = $bunCacheDir
$env:BUN_INSTALL_CACHE_DIR = Join-Path $bunCacheDir "install-cache"

Write-Host "Installing pi-bridge dependencies..."
& $bunExe install
if ($LASTEXITCODE -ne 0) { throw "bun install failed" }

Write-Host "Bundling pi-bridge with --target bun..."
& $bunExe build --target bun src/index.ts --outfile pi-bridge.js
if ($LASTEXITCODE -ne 0) { throw "bun build --target bun failed" }
Pop-Location

Write-Host "Copying Bun runtime and pi-bridge bundle into package..."
Copy-Item $bunExe $package
Copy-Item (Join-Path $bridgeStage "pi-bridge.js") "$package\pi-bridge.js"

$wixZip = Join-Path $tools "wix311-binaries.zip"
$wixDir = Join-Path $tools "wix"
if (-not (Test-Path $wixZip)) {
    Write-Host "Downloading WiX v3.11.2 binaries ..."
    Invoke-WebRequest -Uri "https://github.com/wixtoolset/wix3/releases/download/wix3112rtm/wix311-binaries.zip" -OutFile $wixZip
}
$heatExe = Join-Path $wixDir "heat.exe"
$candleExe = Join-Path $wixDir "candle.exe"
$lightExe = Join-Path $wixDir "light.exe"
if (-not (Test-Path $heatExe) -or -not (Test-Path $candleExe) -or -not (Test-Path $lightExe)) {
    New-Item -ItemType Directory -Force -Path $wixDir | Out-Null
    Expand-Archive -Path $wixZip -DestinationPath $wixDir -Force
}
$env:PATH = "$wixDir;$env:PATH"

$filesWxs = Join-Path $wixOut "files.wxs"
Write-Host "Harvesting staged files with heat..."
& heat dir "$package" -cg ApplicationFiles -gg -sfrag -srd -sreg -dr INSTALLFOLDER -out "$filesWxs"
if ($LASTEXITCODE -ne 0) { throw "heat failed" }

Write-Host "Compiling MSI..."
& candle -arch x64 -out "$wixOut\" (Join-Path $root "wix\main.wxs") "$filesWxs"
if ($LASTEXITCODE -ne 0) { throw "candle failed" }

& light -b "$package" -out "$msiOut" "$wixOut\main.wixobj" "$wixOut\files.wixobj"
if ($LASTEXITCODE -ne 0) { throw "light failed" }

Write-Host "Installer created: $msiOut"
