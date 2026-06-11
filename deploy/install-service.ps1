# install-service.ps1  (ASCII-only to avoid PS5.1 UTF-8-without-BOM parse errors)
#
# Run this script with Administrator privileges.
# Place it next to backend.exe + .env + static\
#
# Usage:
#   .\install-service.ps1            # Fresh install
#   .\install-service.ps1 -Update    # Update binary (stop -> replace -> start)
#   .\install-service.ps1 -Remove    # Unregister service
#
# Requires NSSM: https://nssm.cc/download  (in PATH or set $NssmPath below)

param(
    [switch]$Update,
    [switch]$Remove
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ==============================================================================
#  CONFIG -- edit to match your server
# ==============================================================================
$ServiceName     = "DashboardAPI"
$ServiceDir      = "C:\services\dashboard-api"
$ExePath         = Join-Path $ServiceDir "backend.exe"
$NssmPath        = "nssm"
$OracleClientBin = "C:\OracleX64\product\11.2.0\client_1\bin"
# ==============================================================================

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

Ensure-Admin

# --- Remove -------------------------------------------------------------------
if ($Remove) {
    if (Service-Exists) {
        Write-Host "Stopping $ServiceName..."
        & $NssmPath stop   $ServiceName confirm
        Write-Host "Removing $ServiceName..."
        & $NssmPath remove $ServiceName confirm
        Write-Host "Service removed."
    } else {
        Write-Host "Service $ServiceName not found."
    }
    exit 0
}

# --- Update -------------------------------------------------------------------
if ($Update) {
    if (-not (Service-Exists)) {
        Write-Error "Service $ServiceName not found. Run install-service.ps1 first."
        exit 1
    }
    $src = Join-Path $PSScriptRoot "backend.exe"
    if (-not (Test-Path $src)) {
        Write-Error "backend.exe not found next to this script: $src"
        exit 1
    }
    Write-Host "Stopping $ServiceName..."
    & $NssmPath stop $ServiceName confirm
    Start-Sleep -Seconds 2
    Write-Host "Replacing binary..."
    Copy-Item -Force $src $ExePath
    Write-Host "Starting $ServiceName..."
    & $NssmPath start $ServiceName
    Write-Host "Update complete."
    Get-Service -Name $ServiceName | Select-Object Name, Status
    exit 0
}

# --- Install ------------------------------------------------------------------
if (Service-Exists) {
    Write-Error "Service $ServiceName already exists. Use -Update to replace binary or -Remove to uninstall."
    exit 1
}

if (-not (Test-Path $ServiceDir)) {
    New-Item -ItemType Directory -Force $ServiceDir | Out-Null
}

$src = $PSScriptRoot
Write-Host "Copying files to $ServiceDir..."
Copy-Item -Force         (Join-Path $src "backend.exe")   $ExePath
Copy-Item -Force         (Join-Path $src ".env")          (Join-Path $ServiceDir ".env")
Copy-Item -Recurse -Force (Join-Path $src "static")       (Join-Path $ServiceDir "static")

Write-Host "Registering Windows Service: $ServiceName..."
& $NssmPath install     $ServiceName $ExePath
& $NssmPath set         $ServiceName AppDirectory    $ServiceDir
& $NssmPath set         $ServiceName AppExit         Default Restart
& $NssmPath set         $ServiceName AppRestartDelay 5000

# Add Oracle client to system PATH if needed (required when ORA_ENABLED=1)
$syspath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
if ($syspath -notlike "*$OracleClientBin*") {
    Write-Host "Adding Oracle client to system PATH: $OracleClientBin"
    [System.Environment]::SetEnvironmentVariable(
        "PATH",
        ($OracleClientBin + ";" + $syspath),
        "Machine"
    )
}

$logDir = Join-Path $ServiceDir "log"
New-Item -ItemType Directory -Force $logDir | Out-Null
& $NssmPath set $ServiceName AppStdout      (Join-Path $logDir "stdout.log")
& $NssmPath set $ServiceName AppStderr      (Join-Path $logDir "stderr.log")
& $NssmPath set $ServiceName AppRotateFiles 1
& $NssmPath set $ServiceName AppRotateBytes 10485760

Write-Host "Starting $ServiceName..."
& $NssmPath start $ServiceName

Write-Host ""
Write-Host "Installation complete."
Get-Service -Name $ServiceName | Select-Object Name, Status
Write-Host ""
Write-Host "Verify: http://127.0.0.1:8090/api/v1/health"
