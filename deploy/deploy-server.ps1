# deploy-server.ps1
# Deploy Dashboard API จาก GitHub โดยตรงบน Windows Server
#
# ─── Usage ────────────────────────────────────────────────────────────────────
#   .\deploy-server.ps1 -Setup    # ครั้งแรก: clone + build + register service
#   .\deploy-server.ps1           # อัปเดต:   git pull + build + restart service
#   .\deploy-server.ps1 -Remove   # ถอน service (ไม่ลบ files)
#
# ─── Prerequisites บน Server ──────────────────────────────────────────────────
#   - Git for Windows       https://git-scm.com/download/win
#   - Rust stable toolchain https://rustup.rs/
#   - MSYS2 (ucrt64)        https://www.msys2.org/   (สำหรับ oracle/windows-sys)
#   - NSSM                  https://nssm.cc/download  → วางใน PATH หรือแก้ $NssmPath
#   - Oracle Instant Client ที่ $OracleClientBin      (ถ้า ORA_ENABLED=1)
#
# ─── Layout ───────────────────────────────────────────────────────────────────
#   $BuildDir   = C:\build\Dashboad_API_rust\   ← git repo + cargo target
#   $ServiceDir = C:\services\dashboard-api\    ← runtime: backend.exe + .env + static\
#
#   .env อยู่ที่ $ServiceDir\.env — สร้างด้วยมือจาก .env.example ก่อน -Setup
#   script ไม่เขียนทับ .env เด็ดขาด
# ──────────────────────────────────────────────────────────────────────────────

param(
    [switch]$Setup,   # ครั้งแรก
    [switch]$Remove   # ถอน service
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ══════════════════════════════════════════════════════════════════════════════
#  CONFIG — แก้ค่าเหล่านี้ตาม server จริง
# ══════════════════════════════════════════════════════════════════════════════
$RepoUrl        = "https://github.com/KookZ-code/Dashboad_API_rust.git"
$BuildDir       = "C:\build\Dashboad_API_rust"
$ServiceDir     = "C:\services\dashboard-api"
$ServiceName    = "DashboardAPI"
$NssmPath       = "nssm"          # หรือ "C:\tools\nssm.exe" ถ้าไม่อยู่ใน PATH
$Msys2Bin       = "C:\msys64\ucrt64\bin"
$OracleClientBin= "C:\OracleX64\product\11.2.0\client_1\bin"
# ══════════════════════════════════════════════════════════════════════════════

$ExeDst  = Join-Path $ServiceDir "backend.exe"
$ExeSrc  = Join-Path $BuildDir   "target\release\backend.exe"
$LogDir  = Join-Path $ServiceDir "log"

# ─── Helpers ──────────────────────────────────────────────────────────────────

function Ensure-Admin {
    $p = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
    if (-not $p.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        Write-Error "Script ต้องรันด้วยสิทธิ์ Administrator"
        exit 1
    }
}

function Service-Exists {
    $null -ne (Get-Service -Name $ServiceName -ErrorAction SilentlyContinue)
}

function Service-Running {
    $svc = Get-Service -Name $ServiceName -ErrorAction SilentlyContinue
    $null -ne $svc -and $svc.Status -eq "Running"
}

function Step($msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }
function Ok($msg)   { Write-Host "  OK  $msg" -ForegroundColor Green }
function Warn($msg) { Write-Host "  !!  $msg" -ForegroundColor Yellow }

# ─── Remove ───────────────────────────────────────────────────────────────────

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

# ─── Shared: git pull / clone + cargo build ───────────────────────────────────

function Run-Build {
    # เพิ่ม MSYS2 ใน PATH สำหรับ session นี้ (oracle crate + windows-sys ต้องการ gcc/dlltool)
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
    cargo build --release
    Pop-Location

    if (-not (Test-Path $ExeSrc)) {
        Write-Error "Build สำเร็จแต่ไม่พบ $ExeSrc"
        exit 1
    }
    Ok "Build complete: $ExeSrc"
}

function Copy-Artifacts {
    Step "Copying artifacts to $ServiceDir"
    New-Item -ItemType Directory -Force $ServiceDir | Out-Null
    New-Item -ItemType Directory -Force $LogDir     | Out-Null

    # binary
    Copy-Item -Force $ExeSrc $ExeDst
    Ok "backend.exe copied"

    # static/ (Swagger UI assets — อยู่ใน git, อัปเดตได้ทุก deploy)
    $staticSrc = Join-Path $BuildDir "static"
    if (Test-Path $staticSrc) {
        Copy-Item -Recurse -Force $staticSrc (Join-Path $ServiceDir "static")
        Ok "static\ synced"
    }

    # .env — ไม่เขียนทับถ้ามีอยู่แล้ว
    $envDst = Join-Path $ServiceDir ".env"
    if (-not (Test-Path $envDst)) {
        $envSrc = Join-Path $BuildDir ".env.example"
        if (Test-Path $envSrc) {
            Copy-Item $envSrc $envDst
            Warn ".env ไม่พบ — คัดลอก .env.example ไปแทน แก้ค่าจริงก่อน start service!"
        } else {
            Warn ".env ไม่พบที่ $envDst — สร้างก่อน start service!"
        }
    } else {
        Ok ".env exists (not overwritten)"
    }
}

# ─── Setup (first deploy) ─────────────────────────────────────────────────────

if ($Setup) {
    Ensure-Admin

    if (Service-Exists) {
        Write-Error "Service $ServiceName มีอยู่แล้ว — ใช้ -Remove ก่อนถ้าต้องการติดตั้งใหม่, หรือรัน script โดยไม่มี flag เพื่ออัปเดต"
        exit 1
    }

    Run-Build
    Copy-Artifacts

    # เพิ่ม Oracle client bin เข้า system PATH (persistent)
    $syspath = [System.Environment]::GetEnvironmentVariable("PATH", "Machine")
    if ($syspath -notlike "*$OracleClientBin*") {
        [System.Environment]::SetEnvironmentVariable("PATH", "$OracleClientBin;$syspath", "Machine")
        Ok "Oracle client added to system PATH"
    }

    Step "Registering Windows Service: $ServiceName"
    & $NssmPath install    $ServiceName $ExeDst
    & $NssmPath set        $ServiceName AppDirectory   $ServiceDir
    & $NssmPath set        $ServiceName AppExit        Default Restart
    & $NssmPath set        $ServiceName AppRestartDelay 5000
    & $NssmPath set        $ServiceName AppStdout      (Join-Path $LogDir "stdout.log")
    & $NssmPath set        $ServiceName AppStderr      (Join-Path $LogDir "stderr.log")
    & $NssmPath set        $ServiceName AppRotateFiles 1
    & $NssmPath set        $ServiceName AppRotateBytes 10485760   # 10 MB

    # ตรวจว่ามี .env ค่าจริงแล้วก่อน start
    $envPath = Join-Path $ServiceDir ".env"
    $envReady = (Test-Path $envPath) -and ((Get-Content $envPath) -match "DB_PASSWORD=\S")
    if (-not $envReady) {
        Warn ".env ยังไม่มีค่าจริง — แก้ $envPath ก่อนแล้วรัน: nssm start $ServiceName"
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

# ─── Update (default: git pull + build + restart) ─────────────────────────────

Ensure-Admin

if (-not (Service-Exists)) {
    Write-Error "Service $ServiceName ไม่พบ — รัน .\deploy-server.ps1 -Setup ก่อน"
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
