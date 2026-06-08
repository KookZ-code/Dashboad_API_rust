use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{errors::AppResult, handlers::AppState, models::ApiResponse, repositories::wb_repo::WbRepo};

#[derive(Deserialize)]
pub struct PackagesQuery { pub date: Option<String> }
#[derive(Deserialize)]
pub struct ReportQuery {
    pub date:     Option<String>,
    pub shift:    Option<String>,
    pub packages: Option<String>,
}

pub async fn get_packages(
    State(s): State<AppState>, Query(q): Query<PackagesQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let date = q.date.as_deref().unwrap_or("");
    if date.len() != 10 || !date.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err(crate::errors::AppError::BadRequest("date (YYYY-MM-DD) required".into()));
    }
    let repo = WbRepo::new(&s.mssql, &s.config.view_name, &s.config.machine_table);
    let data = repo.packages(date).await?;
    Ok(Json(ApiResponse::success(data)))
}

pub async fn get_report(
    State(s): State<AppState>, Query(q): Query<ReportQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let date = q.date.as_deref().unwrap_or("");
    if date.len() != 10 || !date.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return Err(crate::errors::AppError::BadRequest("date (YYYY-MM-DD) required".into()));
    }
    let shift    = q.shift.as_deref().unwrap_or("Night");
    let packages = q.packages.as_deref().unwrap_or("__ALL__");
    let repo = WbRepo::new(&s.mssql, &s.config.view_name, &s.config.machine_table);
    let data = repo.report(date, shift, packages).await?;
    Ok(Json(ApiResponse::success(data)))
}
