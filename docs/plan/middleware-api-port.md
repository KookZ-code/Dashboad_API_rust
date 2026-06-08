# แผนงาน: Port Dashboard API (TypeScript → Rust/Axum)

## ที่มา
ต้นทาง: `D:\claude\Project\dashboard-api` — Node.js/Fastify/TypeScript เชื่อม SQL Server  
ปลายทาง: `d:\claude\Dashboad_API_rush` — Rust/Axum/SQLx (ปัจจุบัน: items CRUD + SQLite)

---

## ประเด็นสำคัญก่อนเริ่ม

### ⚠️ SQLx ไม่รองรับ MSSQL
CLAUDE.md ระบุ SQLx แต่ SQLx รองรับเฉพาะ PostgreSQL / MySQL / SQLite  
ต้องใช้ **tiberius** (async MSSQL driver สำหรับ Rust) + **bb8** (connection pool)  
→ **ต้องลบ sqlx ออกจาก Cargo.toml และแทนด้วย tiberius + bb8**

---

## Endpoint ที่ต้องสร้างทั้งหมด

| Module | Endpoint | Method | หมายเหตุ |
|---|---|---|---|
| master | `/api/v1/areas` | GET | รายชื่อ area ทั้งหมด |
| master | `/api/v1/machines` | GET | ?area=&key_only= |
| master | `/api/v1/machines/detail` | GET | ?id= |
| master | `/api/v1/machines/records` | GET | ?id=&limit= |
| overview | `/api/v1/overview` | GET | KPI ปัจจุบัน |
| overview | `/api/v1/overview/open-jobs` | GET | งานที่ยังเปิดอยู่ |
| utilization | `/api/v1/utilization/detail` | GET | ใหญ่สุด: 8 parallel queries |
| utilization | `/api/v1/utilization/by-machine` | GET | |
| utilization | `/api/v1/utilization/attention` | GET | Top 10 ปัญหา |
| downtime | `/api/v1/downtime/*` | GET | ต้องอ่านไฟล์เพิ่ม |
| inventory | `/api/v1/inventory/*` | GET | ต้องอ่านไฟล์เพิ่ม |
| tech | `/api/v1/tech/*` | GET | ต้องอ่านไฟล์เพิ่ม |
| wb | `/api/v1/wb/*` | GET | ต้องอ่านไฟล์เพิ่ม |
| da | `/api/v1/da/*` | GET | ต้องอ่านไฟล์เพิ่ม |
| health | `/api/v1/health` | GET | ไม่ต้อง auth |

---

## สถาปัตยกรรมที่เปลี่ยนใหม่

```
src/
├── main.rs               — bootstrap (เหมือนเดิม)
├── app.rs                — router (เพิ่ม routes ใหม่)
├── config/mod.rs         — เพิ่ม db fields (server, port, name, user, pass, view, machine_table)
├── db/mod.rs             — เปลี่ยน: tiberius + bb8 แทน sqlx sqlite
├── errors/mod.rs         — เหมือนเดิม
├── middleware/
│   ├── mod.rs            — เหมือนเดิม (CORS, RequestId, Trace)
│   └── api_key.rs        — **ใหม่**: ตรวจ x-api-key header
├── helpers/              — **ใหม่**: logic จาก helpers.ts
│   ├── mod.rs
│   ├── where_builder.rs  — buildWhere, buildTechWhere (สร้าง parameterized SQL)
│   └── kpi.rs            — computeKpis, areaUtil, monthlyTrend, r2, nDays
├── handlers/
│   ├── mod.rs
│   ├── health.rs         — เหมือนเดิม (ปรับ db check ใหม่)
│   ├── master.rs         — **ใหม่**
│   ├── overview.rs       — **ใหม่**
│   ├── utilization.rs    — **ใหม่**
│   ├── downtime.rs       — **ใหม่**
│   ├── inventory.rs      — **ใหม่**
│   ├── tech.rs           — **ใหม่**
│   ├── wb.rs             — **ใหม่**
│   └── da.rs             — **ใหม่**
└── repositories/
    ├── mod.rs
    ├── master_repo.rs    — **ใหม่**
    ├── overview_repo.rs  — **ใหม่**
    ├── utilization_repo.rs — **ใหม่** (ใหญ่สุด)
    ├── downtime_repo.rs  — **ใหม่**
    ├── inventory_repo.rs — **ใหม่**
    ├── tech_repo.rs      — **ใหม่**
    ├── wb_repo.rs        — **ใหม่**
    └── da_repo.rs        — **ใหม่**
```

> `items` handler/repo เดิมสามารถลบหรือเก็บไว้ก็ได้ — ขึ้นอยู่กับว่าต้องการหรือไม่

---

## Cargo.toml — Dependencies ที่เปลี่ยน

```toml
# ลบออก
# sqlx = ...

# เพิ่มเข้า
tiberius = { version = "0.12", features = ["chrono", "tds73", "vendored-openssl"] }
bb8      = "0.8"
bb8-tiberius = "0.1"   # หรือใช้ deadpool-tiberius
```

Dependencies อื่น (axum, tokio, serde, tracing, thiserror, dotenvy, chrono, uuid) ยังคงเดิม

---

## Config Struct — Fields ที่ต้องเพิ่ม

```rust
pub struct Config {
    // เดิม
    pub port: u16,
    pub environment: String,
    pub frontend_origin: String,
    // ใหม่
    pub db_server:        String,
    pub db_port:          u16,
    pub db_name:          String,
    pub db_user:          String,
    pub db_password:      String,
    pub api_key:          String,   // "" = ปิด auth
    pub view_name:        String,   // vw_job_nokey
    pub machine_table:    String,   // dbo.machine
}
```

---

## .env.example — Variables ที่ต้องเพิ่ม

```env
DB_SERVER=mth-cl-mthsql
DB_PORT=1433
DB_NAME=MTHAI_ppm_db1
DB_USER=MTHAI_ppm
DB_PASSWORD=

API_KEY=mch_dev_12345
VIEW_NAME=vw_job_nokey
MACHINE_TABLE=dbo.machine
```

---

## API Key Middleware

TypeScript ใช้ `preHandler` per-route  
Rust จะใช้ Axum **layer** ที่ครอบทุก route ยกเว้น `/api/v1/health`:

```rust
// middleware/api_key.rs
// ถ้า config.api_key ไม่ว่าง ตรวจ x-api-key header
// ถ้าไม่ตรง → 401 JSON: { data: null, error: { code: "UNAUTHORIZED", message: "..." } }
```

---

## Response Contract (เหมือน TypeScript ต้นทาง)

```json
// success
{ "status": "ok", "data": { ... } }

// error
{ "status": "error", "error": { "code": 401, "message": "Invalid API key" } }
```

> หมายเหตุ: TypeScript ใช้ `{ status: "ok", data: ... }` ไม่ใช่ `{ data: ..., error: null }` ตาม CLAUDE.md  
> ควรตกลงกับ frontend ว่าจะใช้รูปแบบไหน ก่อนเริ่ม implement

---

## ขั้นตอนการทำงาน (เรียงตามลำดับ)

### Phase 1 — Infrastructure (ไม่เขียน business logic)
1. เพิ่ม `tiberius` + `bb8` ใน Cargo.toml, ลบ `sqlx`
2. แก้ `src/config/mod.rs` — เพิ่ม DB fields
3. แก้ `src/db/mod.rs` — สร้าง `bb8::Pool<tiberius>` แทน SqlitePool
4. แก้ `AppState` ใน `main.rs`
5. แก้ `src/handlers/health.rs` — health check ใช้ `SELECT 1` ผ่าน tiberius
6. **verify**: `cargo check` ผ่าน

### Phase 2 — API Key Middleware
7. สร้าง `src/middleware/api_key.rs`
8. ลงทะเบียนใน `app.rs`
9. **verify**: `cargo check` + test ด้วย curl

### Phase 3 — Helpers
10. สร้าง `src/helpers/where_builder.rs` — port `buildWhere`, `buildTechWhere`
11. สร้าง `src/helpers/kpi.rs` — port `computeKpis`, `areaUtil`, `monthlyTrend`, `r2`, `nDays`
12. **verify**: unit tests สำหรับ `computeKpis`, `r2`

### Phase 4 — Routes (ทำทีละ module, test ทีละอัน)
13. master → handler + repo → test
14. overview → handler + repo → test
15. utilization → handler + repo → test (ใหญ่สุด)
16. downtime → handler + repo → test
17. inventory → handler + repo → test
18. tech → handler + repo → test
19. wb → handler + repo → test
20. da → handler + repo → test

### Phase 5 — Cleanup
21. ลบ items handler/repo (ถ้าไม่ใช้)
22. ลบ migration SQLite (ถ้าไม่ใช้)
23. อัปเดต `.env.example`
24. `cargo clippy --all-targets -- -D warnings`
25. `cargo fmt --check`

---

## ความเสี่ยงและข้อควรระวัง

| ความเสี่ยง | แนวทางรับมือ |
|---|---|
| tiberius parameterized query ใช้ `@p1`, `@p2` (ไม่ใช่ `$1`) | ปรับ buildWhere ให้ generate `@p1`-style |
| ไม่มี compile-time SQL check (ต่างจาก sqlx) | ต้องทดสอบ query ทุกอันกับ DB จริง |
| tiberius ต้องการ tokio `rt-multi-thread` | ตรวจว่า tokio feature ครอบ |
| WB route อาจมี logic พิเศษ (L/R machine) | อ่านไฟล์ wb.ts ก่อน implement |
| TZ = Asia/Bangkok | ตั้ง `TZ` env หรือ convert timestamp ใน Rust |

---

## คำถามที่ต้องตอบก่อนเริ่ม Phase 4

1. **Response format**: ใช้ `{ status, data }` เหมือน TypeScript หรือ `{ data, error }` ตาม CLAUDE.md?
2. **items CRUD**: เก็บไว้หรือลบออก?
3. **debug route**: port มาด้วยหรือไม่? (อาจเปิดเผย internal info)
4. **migrations**: ยังใช้ SQLite + migrate สำหรับอะไร หรือลบออกทั้งหมด?
