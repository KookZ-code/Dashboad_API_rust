// WB-UPH handlers — thin HTTP adapters over wb_uph_repo (SQLite central.db).
//
// rusqlite is synchronous, so each repo call runs inside spawn_blocking. All
// endpoints return RAW numbers; the frontend layers the Excel plan on top.
//
// Query params (frontend already maps display→db package keys and joins with ','):
//   date    YYYY-MM-DD            (defaults to current local date)
//   shift   D | N                 (defaults to current shift)
//   packages comma-separated keys (optional filter; absent/empty = all)
//   hour    integer hour-of-day   (packages/machines; defaults to last shift hour)
//   package single db key         (machines/records; required)
//   machine_id                    (records; required)

use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    errors::{AppError, AppResult},
    handlers::AppState,
    models::ApiResponse,
    repositories::wb_uph_repo as repo,
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

/// Run a synchronous repo closure on the blocking pool and flatten the result.
async fn blocking<F>(f: F) -> AppResult<Json<ApiResponse<Value>>>
where
    F: FnOnce() -> anyhow::Result<Value> + Send + 'static,
{
    let data = tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("blocking task failed: {e}")))??;
    Ok(Json(ApiResponse::success(data)))
}

pub async fn get_summary(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let pkgs = parse_packages(q.packages.as_deref());
    let path = s.config.central_db_path.clone();
    blocking(move || repo::query_summary(&path, &date, &shift, &pkgs)).await
}

pub async fn get_hourly(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let pkgs = parse_packages(q.packages.as_deref());
    let path = s.config.central_db_path.clone();
    blocking(move || repo::query_hourly(&path, &date, &shift, &pkgs)).await
}

pub async fn get_packages(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let pkgs = parse_packages(q.packages.as_deref());
    let hour = q.hour;
    let path = s.config.central_db_path.clone();
    blocking(move || repo::query_packages(&path, &date, &shift, hour, &pkgs)).await
}

pub async fn get_machines(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let package = q.package.clone().filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::BadRequest("package required".into()))?;
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let hour = q.hour;
    let path = s.config.central_db_path.clone();
    blocking(move || repo::query_machines(&path, &date, &shift, hour, &package)).await
}

pub async fn get_records(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let machine_id = q.machine_id.clone().filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::BadRequest("machine_id required".into()))?;
    let package = q.package.clone().filter(|p| !p.is_empty())
        .ok_or_else(|| AppError::BadRequest("package required".into()))?;
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let path = s.config.central_db_path.clone();
    blocking(move || repo::query_records(&path, &date, &shift, &machine_id, &package)).await
}

pub async fn get_monitor(
    State(s): State<AppState>, Query(q): Query<ShiftQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let (date, shift) = repo::resolve_shift(q.date.as_deref(), q.shift.as_deref());
    let path = s.config.central_db_path.clone();
    blocking(move || repo::query_monitor(&path, &date, &shift)).await
}
