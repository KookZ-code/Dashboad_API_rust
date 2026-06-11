# Deploy Guide — Dashboard API (Rust/Axum)

## ภาพรวม

```
Windows Server
─────────────────────────────────────────────────────────────────
C:\build\Dashboad_API_rust\        ← git repo + cargo build (deploy-server.ps1 จัดการ)
C:\services\dashboard-api\         ← runtime (service ชี้มาที่นี่)
  ├── backend.exe                  ← copy จาก target\release\
  ├── .env                        ← สร้างด้วยมือจาก .env.example (ไม่อยู่ใน git)
  ├── static\                     ← sync จาก repo ทุก deploy
  └── log\                        ← NSSM rotate logs

Internet ──► IIS ──► URL Rewrite ──► http://127.0.0.1:8090
```

### deploy-server.ps1 — script หลัก

| Command | ทำอะไร |
|---|---|
| `.\deploy-server.ps1 -Setup` | ครั้งแรก: clone repo → build → register service |
| `.\deploy-server.ps1` | อัปเดต: git pull → build → stop → copy → start |
| `.\deploy-server.ps1 -Remove` | ถอน service (ไม่ลบไฟล์) |

---

## ขั้นตอน 1 — เตรียม Build Machine (ทำครั้งแรกครั้งเดียว)

### 1.1 ติดตั้ง Rust
```powershell
# ดาวน์โหลด rustup จาก https://rustup.rs/ แล้วรัน
rustup default stable
rustup target add x86_64-pc-windows-msvc   # ถ้าใช้ MSVC toolchain
```

### 1.2 MSYS2 (GNU toolchain — ต้องใช้เพราะ oracle crate compile ODPI-C)
```powershell
# ติดตั้ง MSYS2 จาก https://www.msys2.org/
# แล้วเพิ่ม PATH ก่อน build ทุกครั้ง:
$env:PATH = "C:\msys64\ucrt64\bin;$env:PATH"
```

### 1.3 Oracle Instant Client บน build machine
oracle crate compile ODPI-C แล้ว link ที่ build time — ต้องมี Oracle header/lib  
ถ้า build machine มี Oracle client อยู่แล้ว (เช่น `C:\OracleX64\...`) ไม่ต้องทำอะไรเพิ่ม

---

## ขั้นตอน 2 — Build Release Binary

```powershell
cd d:\claude\Dashboad_API_rush

# ต้องมี MSYS2 ใน PATH ก่อน (oracle crate / windows-sys ต้องการ gcc + dlltool)
$env:PATH = "C:\msys64\ucrt64\bin;$env:PATH"

cargo build --release
```

Output: `target\release\backend.exe` (~20-30 MB, static linked)

> **หมายเหตุ:** `migrations/` ถูก embed ใน binary ตอน compile แล้ว — ไม่ต้องคัดลอกไปด้วย

---

## ขั้นตอน 3 — เตรียม Server (ทำครั้งแรกครั้งเดียว)

### 3.1 NSSM
ดาวน์โหลด [nssm.cc/download](https://nssm.cc/download) แล้ววาง `nssm.exe` ใน `C:\Windows\System32\` หรืออัปเดต PATH

### 3.2 Oracle Instant Client บน Server
oracle crate ใช้ ODPI-C **thick mode** — ต้องมี Oracle client DLL ที่ server ด้วย:
```
C:\OracleX64\product\11.2.0\client_1\bin\oci.dll   ← ต้องมีไฟล์นี้
```
install-service.ps1 จะเพิ่ม path นี้เข้า system PATH อัตโนมัติ  
ถ้าไม่ใช้ Oracle ให้ตั้ง `ORA_ENABLED=0` ใน `.env` (ข้ามขั้นตอนนี้ได้)

### 3.3 Network Share Access
Service account ต้องอ่าน `\\mth-sv-file\wire bond\UPH mornitor\central.db` ได้  
ตรวจสอบด้วย:
```powershell
Test-Path "\\mth-sv-file\wire bond\UPH mornitor\central.db"
```
ถ้า service รันใต้ SYSTEM account แต่ share ต้อง authenticate — ให้เปลี่ยน service account:
```powershell
nssm set DashboardAPI ObjectName DOMAIN\serviceuser password
```

### 3.4 IIS URL Rewrite + ARR
1. ติดตั้ง **URL Rewrite Module 2.x** และ **ARR 3.x** จาก Web Platform Installer
2. เปิด IIS Manager → **Application Request Routing Cache** → **Server Proxy Settings** → เปิด **Enable proxy**
3. นำ rules จาก `deploy\iis-rewrite.xml` ใส่ `web.config` ของ site

---

## ขั้นตอน 4 — เตรียมไฟล์บน Server

สร้าง `.env` production ที่ server (ห้าม copy .env จาก dev — password ต่างกัน):

```env
# C:\services\dashboard-api\.env

DATABASE_URL=sqlite:///C:/services/dashboard-api/prod.db
CENTRAL_DB_PATH='\\mth-sv-file\wire bond\UPH mornitor\central.db'
DA_DB_URL=postgres://uph_app:uphpass@MTH-DK-B12416:5432/uph

DB_SERVER=mth-cl-mthsql
DB_PORT=1433
DB_NAME=MTHAI_ppm_db1
DB_USER=MTHAI_ppm
DB_PASSWORD=<production_password>

API_KEY=<strong_random_key>          # สร้างด้วย: -join ((48..57)+(65..90)+(97..122) | Get-Random -Count 32 | % {[char]$_})
VIEW_NAME=vw_job_nokey
MACHINE_TABLE=dbo.machine

ORA_ENABLED=1
ORA_USER=B04469
ORA_PASSWORD=<oracle_password>
ORA_DSN=mth-vm-eqmtaitp.microchip.com:1521/EQMTAIFP
ORA_CLIENT_LIB='C:\OracleX64\product\11.2.0\client_1\bin'
ORA_VIEW=Vw_Asodowntime_2025on
ORA_LIVE_VIEW=EQ_USER.V_EQDOWNTIME

PORT=8090
ENVIRONMENT=production
FRONTEND_ORIGIN=https://your-domain.com   # URL จริงของ frontend
RUST_LOG=warn,backend=info
```

---

## ขั้นตอน 5 — ติดตั้ง Service

คัดลอกไฟล์ที่จำเป็นไปที่ `deploy\` บน build machine:
```
deploy\
├── backend.exe          ← copy จาก target\release\backend.exe
├── .env                 ← .env production ที่เตรียมไว้
├── static\              ← copy โฟลเดอร์ static\ ทั้งหมด
├── install-service.ps1  ← อยู่ใน repo แล้ว
└── iis-rewrite.xml      ← อยู่ใน repo แล้ว
```

รัน script (ต้องการ Administrator):
```powershell
cd deploy
.\install-service.ps1
```

ตรวจสอบ:
```powershell
# Service ขึ้นไหม?
Get-Service DashboardAPI

# Health check
Invoke-WebRequest http://127.0.0.1:8090/api/v1/health | ConvertFrom-Json

# Log
Get-Content "C:\services\dashboard-api\log\stdout.log" -Tail 30
```

---

## ขั้นตอน 6 — อัปเดต Binary (ครั้งถัดไป)

```powershell
# 1. Build ใหม่บน dev machine
$env:PATH = "C:\msys64\ucrt64\bin;$env:PATH"
cargo build --release

# 2. คัดลอก binary ใหม่ไปที่ deploy\
Copy-Item target\release\backend.exe deploy\backend.exe

# 3. รัน update บน server
.\deploy\install-service.ps1 -Update
```

`-Update` จะ stop → replace binary → start อัตโนมัติ (~3 วินาที downtime)

---

## Checklist ก่อน Go-Live

- [ ] `cargo build --release` ผ่านสมบูรณ์ (ไม่มี error)
- [ ] `backend.exe` รันบน dev machine ได้ด้วย production `.env`
- [ ] `http://127.0.0.1:8090/api/v1/health` คืน `{"status":"ok"}`
- [ ] Oracle client ติดตั้งบน server + PATH ตั้งแล้ว (ถ้า `ORA_ENABLED=1`)
- [ ] `\\mth-sv-file\wire bond\...` เข้าได้จาก service account
- [ ] PostgreSQL `MTH-DK-B12416:5432` เข้าได้จาก server
- [ ] NSSM ติดตั้งแล้ว
- [ ] IIS URL Rewrite + ARR ติดตั้งและ enable proxy แล้ว
- [ ] `API_KEY` ใน `.env` ตั้งค่าแล้ว (ไม่ใช่ว่าง)
- [ ] `FRONTEND_ORIGIN` ตรงกับ URL จริงของ frontend (CORS)
- [ ] `.env` ไม่อยู่ใน git (ตรวจด้วย `git status`)
