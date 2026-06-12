use tiberius::{Query, Row};

use crate::{db::MssqlPool, errors::AppError};

/// Run a parameterized SQL query, return first result set.
/// Parameters map to @p1, @p2, ... positionally.
pub async fn exec(pool: &MssqlPool, sql: &str, params: &[String]) -> Result<Vec<Row>, AppError> {
    let mut conn = pool.get().await?;
    let mut q = Query::new(sql);
    for p in params {
        q.bind(p.as_str());
    }
    let rows = q.query(&mut *conn).await?.into_first_result().await?;
    Ok(rows)
}

// ─── Row extraction helpers ───────────────────────────────────────────────────
//
// ใช้ try_get() แทน get() เพื่อป้องกัน panic เมื่อ SQL type ไม่ตรง
// try_get() คืน Result<Option<T>>  →  .ok().flatten() → Option<T>
//   Ok(Some(v)) → Some(v)     (มีค่า)
//   Ok(None)    → None        (NULL)
//   Err(_)      → None        (type mismatch — ลอง type ถัดไป)

pub fn str_val(row: &Row, col: &str) -> String {
    row.try_get::<&str, _>(col).ok().flatten().unwrap_or("").to_string()
}

pub fn opt_str(row: &Row, col: &str) -> Option<String> {
    row.try_get::<&str, _>(col).ok().flatten().map(|s| s.to_string())
}

/// ลอง f64 → f32 → i64 → i32 ตามลำดับ (SQL Server คืน INT/BIGINT/FLOAT/REAL ต่างกัน)
pub fn f64_val(row: &Row, col: &str) -> f64 {
    row.try_get::<f64, _>(col).ok().flatten()
        .or_else(|| row.try_get::<f32, _>(col).ok().flatten().map(|v| v as f64))
        .or_else(|| row.try_get::<i64, _>(col).ok().flatten().map(|v| v as f64))
        .or_else(|| row.try_get::<i32, _>(col).ok().flatten().map(|v| v as f64))
        .unwrap_or(0.0)
}

pub fn i64_val(row: &Row, col: &str) -> i64 {
    row.try_get::<i64, _>(col).ok().flatten()
        .or_else(|| row.try_get::<i32, _>(col).ok().flatten().map(|v| v as i64))
        .unwrap_or(0)
}

pub fn i32_val(row: &Row, col: &str) -> i32 {
    row.try_get::<i32, _>(col).ok().flatten()
        .or_else(|| row.try_get::<i64, _>(col).ok().flatten().map(|v| v as i32))
        .or_else(|| row.try_get::<bool, _>(col).ok().flatten().map(|v| if v { 1 } else { 0 }))
        .or_else(|| row.try_get::<u8, _>(col).ok().flatten().map(|v| v as i32))
        .unwrap_or(0)
}

pub fn opt_dt_str(row: &Row, col: &str) -> Option<String> {
    row.try_get::<chrono::NaiveDateTime, _>(col).ok().flatten()
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

pub fn dt_str_or_empty(row: &Row, col: &str) -> String {
    opt_dt_str(row, col).unwrap_or_default()
}
