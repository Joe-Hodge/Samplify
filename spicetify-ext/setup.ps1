# Samplify - Spicetify integration setup.
#
# RUN THIS in a normal PowerShell window,
# so everything installs against your real Spotify.
#   Right-click this file -> "Run with PowerShell", OR from PowerShell:
#     powershell -ExecutionPolicy Bypass -File .\setup.ps1
#
# Needs the STANDALONE Spotify from spotify.com (NOT the Microsoft Store version -
# Spicetify cannot patch the Store build). This installs it if it is missing.

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "=== Samplify + Spicetify setup ===" -ForegroundColor Cyan

# 0. Warn if the Microsoft Store Spotify is present (Spicetify can't patch it).
$store = Get-AppxPackage -Name "*Spotify*" -ErrorAction SilentlyContinue
if ($store) {
    Write-Host "WARNING: Microsoft Store Spotify detected. Spicetify cannot patch it." -ForegroundColor Red
    Write-Host "Uninstall it (Settings > Apps), install standalone Spotify from spotify.com, then re-run this." -ForegroundColor Red
    Read-Host "Press Enter to continue anyway, or Ctrl+C to stop"
}

# 1. Standalone Spotify (install if absent).
$spotify = Join-Path $env:APPDATA "Spotify\Spotify.exe"
if (-not (Test-Path $spotify)) {
    Write-Host "Standalone Spotify not found. Downloading the official installer..." -ForegroundColor Yellow
    $stub = Join-Path $env:TEMP "SpotifySetup.exe"
    Invoke-WebRequest "https://download.scdn.co/SpotifySetup.exe" -OutFile $stub -UseBasicParsing
    Write-Host "Finish the Spotify installer window, then this continues..." -ForegroundColor Yellow
    Start-Process $stub -Wait
    Start-Sleep 3
}
Write-Host ("Spotify: " + $spotify) -ForegroundColor Gray

# Use the same tested release and cache as Settings > Set up or repair Spotify.
$spiceVersion = "2.46.7"
$archiveHash = "c5a723d31eae529ba13f6922a7a027124238c7f3da7edd182a73eab03c447e9e"
$toolDir = Join-Path $env:LOCALAPPDATA "Samplify/tools/$spiceVersion"
$spice = Join-Path $toolDir "spicetify.exe"
if (-not (Test-Path -LiteralPath $spice)) {
    New-Item -ItemType Directory -Force -Path $toolDir | Out-Null
    $zip = Join-Path $toolDir "cli.zip"
    $url = "https://github.com/spicetify/cli/releases/download/v$spiceVersion/spicetify-$spiceVersion-windows-x64.zip"
    Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip
    if ((Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash -ne $archiveHash) {
        throw "Spicetify download did not match the release checksum."
    }
    Expand-Archive -LiteralPath $zip -DestinationPath $toolDir -Force
    Remove-Item -LiteralPath $zip
}
function Invoke-Spicetify {
    & $spice @args
    if ($LASTEXITCODE -ne 0) { throw "Spicetify failed ($LASTEXITCODE): $args" }
}

# 3. Install the Samplify extension (this script sits next to samplify.js).
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$extDir = Join-Path $env:APPDATA "spicetify\Extensions"
New-Item -ItemType Directory -Force $extDir | Out-Null
Copy-Item (Join-Path $here "samplify.js") (Join-Path $extDir "samplify.js") -Force
Write-Host ("Installed extension -> " + (Join-Path $extDir "samplify.js")) -ForegroundColor Gray

# 4. Patch Spotify + enable the extension.
$spotifyRoot = Split-Path -Parent $spotify
Invoke-Spicetify config spotify_path $spotifyRoot
Invoke-Spicetify config extensions samplify.js
if (Test-Path -LiteralPath (Join-Path $spotifyRoot "Apps/xpui.spa")) {
    Invoke-Spicetify backup apply
} else {
    Invoke-Spicetify apply
}

Write-Host ""
Write-Host "Done. Start the Samplify desktop app, then open Spotify." -ForegroundColor Green
Write-Host "Look for the Capture button in the playbar and note markers on the seekbar." -ForegroundColor Green
Write-Host "After a Spotify update, run this setup again if the Capture button disappears." -ForegroundColor DarkGray
