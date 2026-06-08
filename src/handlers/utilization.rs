use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::utilization_repo::UtilizationRepo};

#[derive(Deserialize)]
pub struct UtilQuery {
    pub start: Option<String>, pub end:   Option<String>,
    pub areas: Option<String>, pub shift: Option<String>,
}

pub async fn get_detail(
    State(s): State<AppState>, Query(q): Query<UtilQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = UtilizationRepo::new(&s.mssql, &s.config.view_name);
    let data = repo.detail(q.start.as_deref(), q.end.as_deref(), q.areas.as_deref(), q.shift.as_deref()).await?;
    Ok(Json(ApiResponse::success(data)))
}

pub async fn get_by_machine(
    State(s): State<AppState>, Query(q): Query<UtilQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = UtilizationRepo::new(&s.mssql, &s.config.view_name);
    let rows = repo.by_machine(q.start.as_deref(), q.end.as_deref(), q.areas.as_deref(), q.shift.as_deref()).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "rows": rows }))))
}

pub async fn get_attention(
    State(s): State<AppState>, Query(q): Query<UtilQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = UtilizationRepo::new(&s.mssql, &s.config.view_name);
    let rows = repo.attention(q.start.as_deref(), q.end.as_deref(), q.areas.as_deref(), q.shift.as_deref()).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "rows": rows }))))
}
