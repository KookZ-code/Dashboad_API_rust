use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::tech_repo::TechRepo};

#[derive(Deserialize)]
pub struct MetricsQuery {
    pub start: Option<String>, pub end: Option<String>,
    pub areas: Option<String>, pub shift: Option<String>,
    pub job_type: Option<String>,
}

pub async fn get_metrics(
    State(s): State<AppState>, Query(q): Query<MetricsQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = TechRepo::new(&s.mssql, &s.config.view_name, &s.oracle);
    let rows = repo.metrics(
        q.start.as_deref(), q.end.as_deref(),
        q.areas.as_deref(), q.shift.as_deref(), q.job_type.as_deref(),
    ).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "rows": rows }))))
}

pub async fn get_list(State(s): State<AppState>) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = TechRepo::new(&s.mssql, &s.config.view_name, &s.oracle);
    let rows = repo.list().await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "rows": rows }))))
}
