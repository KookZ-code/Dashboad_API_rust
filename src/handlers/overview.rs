use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::overview_repo::OverviewRepo};

#[derive(Deserialize)]
pub struct OverviewQuery { pub areas: Option<String> }
#[derive(Deserialize)]
pub struct OpenJobsQuery { pub areas: Option<String>, pub job_type: Option<String> }

pub async fn get_overview(
    State(s): State<AppState>,
    Query(q): Query<OverviewQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = OverviewRepo::new(&s.mssql, &s.config.machine_table);
    let data = repo.kpi_and_matrix(q.areas.as_deref()).await?;
    Ok(Json(ApiResponse::success(data)))
}

pub async fn get_open_jobs(
    State(s): State<AppState>,
    Query(q): Query<OpenJobsQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = OverviewRepo::new(&s.mssql, &s.config.machine_table);
    let jobs = repo.open_jobs(q.areas.as_deref(), q.job_type.as_deref()).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "jobs": jobs }))))
}
