# install-service.ps1
# รัน script นี้บน Server ด้วยสิทธิ์ Administrator
# วาง script นี้ไว้ในโฟลเดอร์เดียวกับ backend.exe + .env + static\
#
# Usage:
#   .\install-service.ps1            # ติดตั้งครั้งแรก
#   .\install-service.ps1 -Update    # อัปเดต binary (stop → replace → start)
#   .\install-service.ps1 -Remove    # ถอนถอน service
#
# ต้องติดตั้ง NSSM ก่อน: https://nssm.cc/download
# ตรวจสอบ nssm อยู่ใน PATH หรือวางที่ deploy\nssm.exe ได้เลย

param(
    [switch]$Update,
    [switch]$Remove
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ─── Config — แก้ค่าเหล่านี้ตาม server จริง ──────────────────────────────────
$ServiceName = "DashboardAPI"
$ServiceDir  = "C:\services\dashboard-api"   # ที่วาง backend.exe + .env + static\
$ExePath     = Join-Path $ServiceDir "backend.exe"
$NssmPath    = "nssm"                         # ถ้าวาง nssm.exe ใน deploy: Join-Path $PSScriptRoot "nssm.exe"

# Oracle client bin ต้องอยู่ใน PATH ของ service ด้วย (ถ้า ORA_ENABLED=1)
$OracleClientBin = "C:\OracleX64\product\11.2.0\client_1\bin"
# ─────────────────────────────────────────────────────────────────────────────

function Ensure-Admin {
    $cur = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]$cur
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Error "Script ต้องรันด้วยสิทธิ์ Administrator"
        exit 1
    }
}

function Service-Exists {
    $svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    return $null -ne $svc
}

Ensure-Admin

# ─── Remove ───────────────────────────────────────────────────────────────────
if ($Remove) {
    if (Service-Exists) {
        Write-Host "Stopping $ServiceName..."
        & $NssmPath stop $ServiceName confirm
        Write-Host "Removing $ServiceName..."
        & $NssmPath remove $ServiceName confirm
        Write-Host "Service removed."
    } else {
        Write-Host "Service $ServiceName not found."
    }
    exit 0
}

# ─── Update (stop → copy new binary → start) ──────────────────────────────────
if ($Update) {
    if (-not (Service-Exists)) {
        Write-Error "Service $ServiceName ไม่พบ — รัน install-service.ps1 ก่อน"
        exit 1
    }
    $src = Join-Path $PSScriptRoot "backend.exe"
    if (-not (Test-Path $src)) {
        Write-Error "ไม่พบ backend.exe ข้าง script นี้ ($src)"
        exit 1
    }
    Write-Host "Stopping $ServiceName..."
    & $NssmPath stop $ServiceName confirm
    Start-Sleep -Seconds 2

    Write-Host "Replacing binary..."
    Copy-Item -Force $src $ExePath

    Write-Host "Starting $ServiceName..."
    & $NssmPath start $ServiceName
    Write-Host "Update complete. Service status:"
    Get-Service -Name $ServiceName | Select-Object Name, Status
    exit 0
}

# ─── Install (fresh) ──────────────────────────────────────────────────────────
if (Service-Exists) {
    Write-Error "Service $ServiceName มีอยู่แล้ว — ใช้ -Update เพื่ออัปเดต binary หรือ -Remove เพื่อถอน"
    exit 1
}

if (-not (Test-Path $ServiceDir)) {
    Write-Host "Creating $ServiceDir..."
    New-Item -ItemType Directory -Force $ServiceDir | Out-Null
}

# คัดลอกไฟล์ที่จำเป็น
$src = $PSScriptRoot
Write-Host "Copying files to $ServiceDir..."
Copy-Item -Force  (Join-Path $src "backend.exe")  $ExePath
Copy-Item -Force  (Join-Path $src ".env")          (Join-Path $ServiceDir ".env")
Copy-Item -Recurse -Force (Join-Path $src "static") (Join-Path $ServiceDir "static")

# ลงทะเบียน service
Write-Host "Registering Windows Service: $ServiceName..."
& $NssmPath install $ServiceName $ExePath

# ตั้ง working directory (dotenvy อ่าน .env จาก cwd)
& $NssmPath set $ServiceName AppDirectory $ServiceDir

# ตั้ง restart policy: รีสตาร์ทอัตโนมัติถ้า crash (delay 5 วินาที)
& $NssmPath set $ServiceName AppExit Default Restart
& $NssmPath set $ServiceName AppRestartDelay 5000

# เพิ่ม Oracle client ใน PATH ของ service (ถ้า ORA_ENABLED=1)
$currentPath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
if ($currentPath -notlike "*$OracleClientBin*") {
    Write-Host "Adding Oracle client to system PATH: $OracleClientBin"
    [System.Environment]::SetEnvironmentVariable("PATH", "$OracleClientBin;$currentPath", "Machine")
}

# Log stdout + stderr ไปที่ log\
$logDir = Join-Path $ServiceDir "log"
New-Item -ItemType Directory -Force $logDir | Out-Null
& $NssmPath set $ServiceName AppStdout (Join-Path $logDir "stdout.log")
& $NssmPath set $ServiceName AppStderr (Join-Path $logDir "stderr.log")
& $NssmPath set $ServiceName AppRotateFiles 1
& $NssmPath set $ServiceName AppRotateBytes 10485760   # rotate ที่ 10 MB

Write-Host "Starting $ServiceName..."
& $NssmPath start $ServiceName

Write-Host ""
Write-Host "Installation complete. Service status:"
Get-Service -Name $ServiceName | Select-Object Name, Status
Write-Host ""
Write-Host "Verify: http://127.0.0.1:8080/api/v1/health"
