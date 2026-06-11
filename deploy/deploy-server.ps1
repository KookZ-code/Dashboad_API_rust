# deploy-server.ps1  (ASCII-only to avoid PS5.1 UTF-8-without-BOM parse errors)
#
# Usage:
#   .\deploy-server.ps1 -Setup    # First deploy: git clone + cargo build + register NSSM service
#   .\deploy-server.ps1           # Update:       git pull + cargo build + restart service
#   .\deploy-server.ps1 -Remove   # Uninstall service (files kept)
#
# Prerequisites on the server:
#   - Git for Windows      https://git-scm.com/download/win
#   - Rust stable          https://rustup.rs/
#   - MSYS2 (ucrt64)       https://www.msys2.org/
#   - NSSM                 https://nssm.cc/download
#   - Oracle Instant Client at $OracleClientBin  (only if ORA_ENABLED=1)
#
# Layout after -Setup:
#   $BuildDir   = C:\build\Dashboad_API_rust\   <- git repo + cargo target
#   $ServiceDir = C:\services\dashboard-api\    <- runtime: backend.exe + .env + static\
#
#   .env lives in $ServiceDir\.env  -- create it manually from .env.example BEFORE -Setup
#   This script never overwrites .env

param(
    [switch]$Setup,
    [switch]$Remove
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ==============================================================================
#  CONFIG  -- edit these to match your server
# ==============================================================================
$RepoUrl         = "https://github.com/KookZ-code/Dashboad_API_rust.git"
$BuildDir        = "C:\build\Dashboad_API_rust"
$ServiceDir      = "C:\services\dashboard-api"
$ServiceName     = "DashboardAPI"
$NssmPath        = "nssm"
$Msys2Bin        = "C:\msys64\ucrt64\bin"
$OracleClientBin = "C:\OracleX64\product\11.2.0\client_1\bin"
# ==============================================================================

$ExeDst = Join-Path $ServiceDir "backend.exe"
$ExeSrc = Join-Path $BuildDir   "target\release\backend.exe"
$LogDir = Join-Path $ServiceDir "log"

# --- helpers ------------------------------------------------------------------

function Ensure-Admin {
    $cur = [Security.Principal.WindowsIdentity]::GetCurrent()
    $p   = [Security.Principal.WindowsPrincipal]$cur
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Error "Run this script as Administrator"
        exit 1
    }
}

function Service-Exists {
    $null -ne (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)
}

function Service-Running {
    $s = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    ($null -ne $s) -and ($s.Status -eq "Running")
}

function Step { param($msg) Write-Host "`n=== $msg ===" -ForegroundColor Cyan }
function Ok   { param($msg) Write-Host "  OK  $msg"    -ForegroundColor Green }
function Warn { param($msg) Write-Host "  !!  $msg"    -ForegroundColor Yellow }

# --- Remove -------------------------------------------------------------------

if ($Remove) {
    Ensure-Admin
    if (Service-Exists) {
        Step "Stopping and removing $ServiceName"
        & $NssmPath stop   $ServiceName confirm
        & $NssmPath remove $ServiceName confirm
        Ok "Service removed (files kept at $ServiceDir)"
    } else {
        Warn "Service $ServiceName not found"
    }
    exit 0
}

# --- Build (shared by Setup and Update) ---------------------------------------

function Run-Build {
    if ($env:PATH -notlike "*$Msys2Bin*") {
        $env:PATH = "$Msys2Bin;$env:PATH"
        Ok "MSYS2 added to PATH"
    }

    if (Test-Path (Join-Path $BuildDir ".git")) {
        Step "git pull"
        Push-Location $BuildDir
        git pull origin main
        Pop-Location
    } else {
        Step "git clone"
        New-Item -ItemType Directory -Force (Split-Path $BuildDir) | Out-Null
        git clone $RepoUrl $BuildDir
    }

    Step "cargo build --release"
    Push-Location $BuildDir
    $env:SQLX_OFFLINE = "true"
    cargo build --release
    Pop-Location

    if (-not (Test-Path $ExeSrc)) {
        Write-Error "Build succeeded but binary not found: $ExeSrc"
        exit 1
    }
    Ok "Build complete: $ExeSrc"
}

function Copy-Artifacts {
    Step "Copying artifacts to $ServiceDir"
    New-Item -ItemType Directory -Force $ServiceDir | Out-Null
    New-Item -ItemType Directory -Force $LogDir     | Out-Null

    Copy-Item -Force $ExeSrc $ExeDst
    Ok "backend.exe copied"

    $staticSrc = Join-Path $BuildDir "static"
    if (Test-Path $staticSrc) {
        Copy-Item -Recurse -Force $staticSrc (Join-Path $ServiceDir "static")
        Ok "static\ synced"
    }

    $envDst = Join-Path $ServiceDir ".env"
    if (-not (Test-Path $envDst)) {
        $envSrc = Join-Path $BuildDir ".env.example"
        if (Test-Path $envSrc) {
            Copy-Item $envSrc $envDst
            Warn ".env not found -- copied .env.example. Edit $envDst before starting the service!"
        } else {
            Warn ".env not found at $envDst -- create it before starting the service!"
        }
    } else {
        Ok ".env exists (not overwritten)"
    }
}

# --- Setup (first deploy) -----------------------------------------------------

if ($Setup) {
    Ensure-Admin

    if (Service-Exists) {
        Write-Error "Service $ServiceName already exists. Use -Remove to uninstall first, or run without flags to update."
        exit 1
    }

    Run-Build
    Copy-Artifacts

    # Add Oracle client bin to system PATH (persistent, required if ORA_ENABLED=1)
    $syspath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
    if ($syspath -notlike "*$OracleClientBin*") {
        [System.Environment]::SetEnvironmentVariable(
            "PATH",
            ($OracleClientBin + ";" + $syspath),
            "Machine"
        )
        Ok "Oracle client added to system PATH"
    }

    Step "Registering Windows Service: $ServiceName"
    & $NssmPath install     $ServiceName $ExeDst
    & $NssmPath set         $ServiceName AppDirectory    $ServiceDir
    & $NssmPath set         $ServiceName AppExit         Default Restart
    & $NssmPath set         $ServiceName AppRestartDelay 5000
    & $NssmPath set         $ServiceName AppStdout       (Join-Path $LogDir "stdout.log")
    & $NssmPath set         $ServiceName AppStderr       (Join-Path $LogDir "stderr.log")
    & $NssmPath set         $ServiceName AppRotateFiles  1
    & $NssmPath set         $ServiceName AppRotateBytes  10485760

    $envPath  = Join-Path $ServiceDir ".env"
    $envReady = (Test-Path $envPath) -and ((Get-Content $envPath -Raw) -match "DB_PASSWORD=\S")
    if (-not $envReady) {
        Warn ".env missing real values -- edit $envPath then run: nssm start $ServiceName"
    } else {
        Step "Starting $ServiceName"
        & $NssmPath start $ServiceName
    }

    Write-Host ""
    Get-Service -Name $ServiceName -ErrorAction SilentlyContinue | Select-Object Name, Status
    Write-Host ""
    Ok "Setup complete. Verify: http://127.0.0.1:8090/api/v1/health"
    exit 0
}

# --- Update (default: git pull + build + restart) -----------------------------

Ensure-Admin

if (-not (Service-Exists)) {
    Write-Error "Service $ServiceName not found. Run .\deploy-server.ps1 -Setup first."
    exit 1
}

Run-Build
Copy-Artifacts

Step "Restarting $ServiceName"
if (Service-Running) {
    & $NssmPath stop $ServiceName confirm
    Start-Sleep -Seconds 2
}
& $NssmPath start $ServiceName

Write-Host ""
Get-Service -Name $ServiceName | Select-Object Name, Status
Write-Host ""
Ok "Deploy complete. Verify: http://127.0.0.1:8090/api/v1/health"
