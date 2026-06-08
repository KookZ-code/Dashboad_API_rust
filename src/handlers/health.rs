use axum::{extract::State, http::StatusCode, Json};
use serde::Serialize;

use crate::db::MssqlPool;

#[derive(Serialize)]
pub struct HealthResponse {
    status: &'static str,
    database: &'static str,
    version: &'static str,
}

/// GET /api/v1/health — ไม่ต้อง auth
pub async fn health_check(State(pool): State<MssqlPool>) -> (StatusCode, Json<HealthResponse>) {
    let db_ok = match pool.get().await {
        Ok(mut conn) => conn.execute("SELECT 1", &[]).await.is_ok(),
        Err(_) => false,
    };

    let (status_code, status, database) = if db_ok {
        (StatusCode::OK, "ok", "healthy")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "degraded", "unhealthy")
    };

    (status_code, Json(HealthResponse { status, database, version: env!("CARGO_PKG_VERSION") }))
}
