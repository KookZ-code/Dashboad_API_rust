use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::inventory_repo::InventoryRepo};

#[derive(Deserialize)]
pub struct MachinesQuery { pub area: Option<String>, pub key_only: Option<String> }

fn repo(s: &AppState) -> InventoryRepo<'_> {
    InventoryRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name, &s.config.job_table)
}

pub async fn get_machines(
    State(s): State<AppState>, Query(q): Query<MachinesQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let ko = matches!(q.key_only.as_deref(), Some("true") | Some("1") | Some("yes"));
    let machines = repo(&s).machines(q.area.as_deref(), ko).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "machines": machines }))))
}

pub async fn get_downtime(
    State(s): State<AppState>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let rows = repo(&s).downtime_summary().await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "rows": rows }))))
}

pub async fn get_last_package(
    State(s): State<AppState>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let rows = repo(&s).last_package().await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "packages": rows }))))
}

pub async fn probe_job_columns(
    State(s): State<AppState>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let rows = repo(&s).probe_job_columns().await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "columns": rows }))))
}
