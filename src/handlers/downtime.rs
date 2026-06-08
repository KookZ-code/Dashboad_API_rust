use axum::{extract::{Query, State}, Json};
use serde::Deserialize;
use serde_json::Value;

use crate::{
    errors::AppResult, handlers::AppState, models::ApiResponse,
    repositories::downtime_repo::{DowntimeDetailOpts, DowntimeEventOpts, DowntimeRepo},
};

#[derive(Deserialize)]
pub struct DetailQuery {
    pub job_types: Option<String>, pub start: Option<String>, pub end: Option<String>,
    pub areas: Option<String>, pub shift: Option<String>,
    pub reason_col: Option<String>, pub limit: Option<String>,
}
#[derive(Deserialize)]
pub struct MachinesQuery { pub areas: Option<String> }
#[derive(Deserialize)]
pub struct EventsQuery {
    pub job_types: Option<String>, pub start: Option<String>, pub end: Option<String>,
    pub areas: Option<String>, pub shift: Option<String>,
    pub machine: Option<String>, pub symptom: Option<String>,
    pub cause: Option<String>, pub tech: Option<String>,
    pub limit: Option<String>,
}

pub async fn get_detail(
    State(s): State<AppState>, Query(q): Query<DetailQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let lim = q.limit.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(20).clamp(5, 50);
    let repo = DowntimeRepo::new(&s.mssql, &s.config.view_name);
    let data = repo.detail(DowntimeDetailOpts {
        job_types: q.job_types.as_deref(), start: q.start.as_deref(),
        end: q.end.as_deref(), areas: q.areas.as_deref(), shift: q.shift.as_deref(),
        reason_col: q.reason_col.as_deref(), limit: lim,
    }).await?;
    Ok(Json(ApiResponse::success(data)))
}

pub async fn get_machines(
    State(s): State<AppState>, Query(q): Query<MachinesQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let repo = DowntimeRepo::new(&s.mssql, &s.config.view_name);
    let machines = repo.machines_with_downtime(q.areas.as_deref()).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "machines": machines }))))
}

pub async fn get_events(
    State(s): State<AppState>, Query(q): Query<EventsQuery>,
) -> AppResult<Json<ApiResponse<Value>>> {
    let lim = q.limit.as_deref().and_then(|s| s.parse::<u32>().ok()).unwrap_or(500).clamp(50, 2000);
    let repo = DowntimeRepo::new(&s.mssql, &s.config.view_name);
    let events = repo.events(DowntimeEventOpts {
        job_types: q.job_types.as_deref(), start: q.start.as_deref(),
        end: q.end.as_deref(), areas: q.areas.as_deref(), shift: q.shift.as_deref(),
        machine: q.machine.as_deref(), symptom: q.symptom.as_deref(),
        cause: q.cause.as_deref(), tech: q.tech.as_deref(), limit: lim,
    }).await?;
    Ok(Json(ApiResponse::success(serde_json::json!({ "events": events, "total": events.len() }))))
}
