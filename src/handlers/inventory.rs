use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::inventory_repo::InventoryRepo};

#[derive(Deserialize)]
pub struct MachinesQuery { pub area: Option<String>, pub key_only: Option<String> }

pub async fn get_machines(
    State(s): State<AppState>, Query(q): Query<MachinesQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let ko = matches!(q.key_only.as_deref(), Some("true") | Some("1") | Some("yes"));
    let repo = InventoryRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name);
    let machines = repo.machines(q.area.as_deref(), ko).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "machines": machines }))))
}

pub async fn get_downtime(
    State(s): State<AppState>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = InventoryRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name);
    let rows = repo.downtime_summary().await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "rows": rows }))))
}
