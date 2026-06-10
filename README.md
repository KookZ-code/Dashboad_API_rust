# Dashboard API — Rust/Axum Backend

REST API middleware สำหรับ **WB Dashboard** — port จาก Node.js/Fastify มาเป็น Rust/Axum เพื่อประสิทธิภาพสูงและ memory footprint ต่ำ

> **Stack:** Rust · Axum 0.8 · Tiberius (MSSQL) · SQLx (SQLite) · bb8 connection pool  
> **Deploy:** Windows Server · NSSM Windows Service · IIS Reverse Proxy

---

## Features

| Module | Endpoints | คำอธิบาย |
|---|---|---|
| **Health** | `GET /api/v1/health` | Server + DB health check |
| **Master** | `GET /api/v1/areas` `/machines` `/machines/detail` `/machines/records` | ข้อมูล Area และเครื่องจักร |
| **Overview** | `GET /api/v1/overview` `/overview/open-jobs` | KPI ภาพรวม real-time |
| **Utilization** | `GET /api/v1/utilization/detail` `/by-machine` `/attention` | วิเคราะห์การใช้งานเครื่อง (8 parallel queries) |
| **Downtime** | `GET /api/v1/downtime/detail` `/machines` `/events` | วิเคราะห์ downtime + Pareto สาเหตุ |
| **Inventory** | `GET /api/v1/inventory/machines` `/downtime` | ทะเบียนเครื่องจักร |
| **Tech** | `GET /api/v1/tech/metrics` `/list` | คะแนน KPI รายช่าง (MTTR, FTFR, Response) |
| **WB Report** | `GET /api/v1/wb/packages` `/report` | รายงาน Wire Bonding รายกะ (utilization + events) |
| **DA Report** | `GET /api/v1/da/packages` `/report` | รายงาน Die Attach รายกะ |
| **WB-UPH** | `GET /api/v1/wb-uph/summary` `/hourly` `/packages` `/machines` `/records` `/monitor` | Wire-Bond hourly **UPH monitor** — อ่านจาก SQLite `central.db` (ไม่ใช่ MSSQL) คืนตัวเลขดิบ (reset-aware delta, MPC key, shift window); plan target คำนวณฝั่ง frontend |
| **Items** | `CRUD /api/v1/items` | Demo CRUD (SQLite) |
| **Docs** | `GET /docs` | Swagger UI |
| **Spec** | `GET /openapi.json` | OpenAPI 3.0 spec |

---

## Architecture

```
HTTP Request
  → Axum Router
  → API Key Middleware (optional)
  → Handler (validate + call repo)
  → Repository (tiberius SQL)
  → MSSQL Server
  ↓
JSON Response { data: T, error: null }
```

**Three data sources:**
- **MSSQL** (tiberius + bb8) — dashboard data จาก SQL Server (overview, utilization, downtime, tech, wb/da report ฯลฯ)
- **SQLite `central.db`** (rusqlite) — Wire-Bond hourly UPH (`wb-uph/*`). อ่านจาก network share โดย **mirror มา local cache** (stale-while-revalidate): เสิร์ฟ cache ทันทีแล้ว refresh ใน background — กัน SQLite-over-SMB ที่ช้า/ล็อก ตั้ง path ที่ `CENTRAL_DB_PATH`
- **SQLite `dev.db`** (sqlx) — items CRUD demo

> `wb-uph` queries เป็น synchronous (rusqlite) จึงรันใน `tokio::task::spawn_blocking`

---

## Requirements

- Rust stable (x86_64-pc-windows-gnu หรือ msvc)
- MSYS2 installed ที่ `C:\msys64` (สำหรับ GNU toolchain)
- SQL Server ที่ accessible จาก machine นี้

---

## Setup

### 1. Clone

```bash
git clone https://github.com/KookZ-code/Dashboad_API_rust.git
cd Dashboad_API_rust
```

### 2. ตั้งค่า Environment

```bash
cp .env.example .env
```

แก้ `.env`:

```env
# SQLite
DATABASE_URL=sqlite://./dev.db

# MSSQL
DB_SERVER=your-sql-server
DB_PORT=1433
DB_NAME=your-database
DB_USER=your-user
DB_PASSWORD=your-password

# API Key (ว่าง = ปิด auth)
API_KEY=

# View/Table names
VIEW_NAME=vw_job_nokey
MACHINE_TABLE=dbo.machine

# WB-UPH: path ไป central.db (network share หรือ local). ถ้า path มีช่องว่างต้องครอบ single quote
CENTRAL_DB_PATH='\\mth-sv-file\wire bond\UPH mornitor\central.db'

# Server
PORT=8080
ENVIRONMENT=development
FRONTEND_ORIGIN=http://localhost:5174
RUST_LOG=warn,backend=info
```

### 3. Build & Run

**GNU toolchain (Windows):**

```powershell
# ต้องเพิ่ม MSYS2 ใน PATH ก่อน (windows-link ต้องการ dlltool)
$env:PATH = "C:\msys64\ucrt64\bin;$env:PATH"

cargo run
```

**MSVC toolchain:**

```powershell
cargo run
```

Server จะเปิดที่ `http://127.0.0.1:8080`

---

## API Documentation

เปิด browser ไปที่:

```
http://127.0.0.1:8080/docs
```

Swagger UI พร้อม Try it out — กด **Authorize** ถ้าเปิด `API_KEY`

---

## Response Format

```json
// Success
{ "data": { ... }, "error": null }

// Error
{ "data": null, "error": { "code": "NOT_FOUND", "message": "..." } }
```

---

## Authentication

ส่ง header `x-api-key: <value>` ทุก request ยกเว้น `/health` และ `/docs`

ถ้า `API_KEY=` ว่างใน `.env` = ปิด auth (dev mode)

---

## Build for Production (Windows Service)

```powershell
$env:PATH = "C:\msys64\ucrt64\bin;$env:PATH"
cargo build --release
```

Binary อยู่ที่ `target\release\backend.exe`

ลงทะเบียน Windows Service ด้วย NSSM:

```powershell
nssm install DashboardAPI "C:\services\dashboard-api\backend.exe"
nssm set DashboardAPI AppDirectory "C:\services\dashboard-api"
nssm start DashboardAPI
```

---

## Project Structure

```
src/
├── main.rs               — Bootstrap
├── app.rs                — Router + Middleware stack
├── config/               — Typed env config
├── db/                   — Pool setup (MSSQL + SQLite)
├── errors/               — AppError → HTTP response
├── handlers/             — HTTP layer (thin adapters)
│   ├── master.rs
│   ├── overview.rs
│   ├── utilization.rs
│   ├── downtime.rs
│   ├── inventory.rs
│   ├── tech.rs
│   ├── wb.rs
│   ├── da.rs
│   ├── wb_uph.rs         — WB-UPH (central.db) handlers
│   └── docs.rs           — Swagger UI
├── repositories/         — SQL queries
│   ├── mssql_util.rs     — try_get helpers
│   ├── master_repo.rs
│   ├── overview_repo.rs
│   ├── utilization_repo.rs
│   ├── downtime_repo.rs
│   ├── inventory_repo.rs
│   ├── tech_repo.rs
│   ├── wb_repo.rs        — WB + DA shift report logic
│   ├── da_repo.rs
│   └── wb_uph_repo.rs    — WB-UPH (rusqlite + central.db mirror)
├── helpers/
│   ├── where_builder.rs  — Parameterized WHERE clause builder
│   └── kpi.rs            — KPI calculations (utilization, MTTR)
└── middleware/
    └── api_key.rs        — x-api-key authentication
static/
├── api-docs.html         — Swagger UI
└── openapi.json          — OpenAPI 3.0 spec
migrations/               — SQLite migrations (items demo)
docs/plan/                — Implementation planning docs
```

---

## Tests

```powershell
$env:PATH = "C:\msys64\ucrt64\bin;$env:PATH"
cargo test
```

26 tests (9 unit + 17 integration) — ครอบ KPI calculations + WHERE builder

---

## License

MIT
