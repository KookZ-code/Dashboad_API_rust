use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::master_repo::MasterRepo};

#[derive(Deserialize)]
pub struct MachinesQuery { pub area: Option<String>, pub key_only: Option<String> }
#[derive(Deserialize)]
pub struct DetailQuery { pub id: Option<String>, pub recent_limit: Option<String> }
#[derive(Deserialize)]
pub struct RecordsQuery { pub id: Option<String>, pub limit: Option<String> }

pub async fn get_areas(State(s): State<AppState>) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = MasterRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name);
    let areas = repo.areas().await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "areas": areas }))))
}

pub async fn get_machines(
    State(s): State<AppState>,
    Query(q): Query<MachinesQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let ko = matches!(q.key_only.as_deref(), Some("true") | Some("1") | Some("yes"));
    let repo = MasterRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name);
    let machines = repo.machines(q.area.as_deref(), ko).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "machines": machines }))))
}

pub async fn get_machine_detail(
    State(s): State<AppState>,
    Query(q): Query<DetailQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let id = q.id.as_deref().unwrap_or("");
    if id.is_empty() { return Err(crate::errors::AppError::BadRequest("id required".into())); }
    let lim = q.recent_limit.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(25);
    let repo = MasterRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name);
    let data = repo.machine_detail(id, lim).await?;
    Ok(Json(ApiResponse::success(data)))
}

pub async fn get_machine_records(
    State(s): State<AppState>,
    Query(q): Query<RecordsQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let id = q.id.as_deref().unwrap_or("");
    if id.is_empty() { return Err(crate::errors::AppError::BadRequest("id required".into())); }
    let lim = q.limit.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(200);
    let repo = MasterRepo::new(&s.mssql, &s.config.machine_table, &s.config.view_name);
    let records = repo.machine_records(id, lim).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "records": records }))))
}
