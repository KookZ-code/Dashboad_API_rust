// DA-UPH handlers — thin async HTTP adapters over da_uph_repo (PostgreSQL `uph`).
//
// Unlike wb_uph (rusqlite → spawn_blocking), sqlx Postgres is async, so these call
// the repo directly with `.await`. The pg pool is Option: when DA_DB_URL is unset or
// the DA workstation was unreachable at startup, every endpoint returns 503 while the
// rest of the API center keeps serving.
//
// Query params mirror wb_uph: date, shift, packages (csv), hour, package, machine_id.

use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    db::PgPool,
    errors::{AppError, AppResult},
    handlers::AppState,
    models::ApiResponse,
    repositories::da_uph_repo as repo,
};

#[derive(Deserialize)]
pub struct ShiftQuery {
    pub date: Option<String>,
    pub shift: Option<String>,
    pub packages: Option<String>,
    pub hour: Option<u32>,
    pub package: Option<String>,
    pub machine_id: Option<String>,
}

fn parse_packages(raw: Option<&str>) -> Vec<String> {
    raw.map(|v| {
        v.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

/// Borrow the DA Postgres pool or fail with 503 (mapped to INTERNAL by AppError).
fn pool(s: &AppState) -> AppResult<&PgPool> {
    s.pg
        .as_ref()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("DA-UPH Postgres unavailable")))
}

fn ok(data: Value) -> Json<ApiResponse<Value>> {
    Json(ApiResponse::success(data))
}

pub async fn get_summary(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let pkgs = parse_packages(q.packages.as_deref());
    Ok(ok(repo::query_summary(pool(&s)?, &date, &shift, &pkgs).await?))
}

pub async fn get_hourly(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let pkgs = parse_packages(q.packages.as_deref());
    Ok(ok(repo::query_hourly(pool(&s)?, &date, &shift, &pkgs).await?))
}

pub async fn get_packages(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let pkgs = parse_packages(q.packages.as_deref());
    Ok(ok(repo::query_packages(pool(&s)?, &date, &shift, q.hour, &pkgs).await?))
}

pub async fn get_machines(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let package = q.package.clone().filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::BadRequest("package required".into()))?;
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    Ok(ok(repo::query_machines(pool(&s)?, &date, &shift, q.hour, &package).await?))
}

pub async fn get_records(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let machine_id = q.machine_id.clone().filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::BadRequest("machine_id required".into()))?;
    let package = q.package.clone().filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::BadRequest("package required".into()))?;
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    Ok(ok(repo::query_records(pool(&s)?, &date, &shift, &machine_id, &package).await?))
}

pub async fn get_monitor(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    Ok(ok(repo::query_monitor(pool(&s)?, &date, &shift).await?))
}
