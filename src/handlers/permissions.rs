use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{handlers::AppState, repositories::auth_repo};

pub async fn get_permissions(State(state): State<AppState>) -> Result<Json<Value>, Json<Value>> {
    let perms = auth_repo::get_permissions(&state.mssql)
        .await
        .map_err(|e| json!({ "data": null, "error": { "code": "DB_ERROR", "message": e.to_string() } }))?;
    Ok(Json(json!({ "data": perms, "error": null })))
}

#[derive(Deserialize)]
pub struct SetPermsReq {
    paths: Vec<String>,
}

pub async fn set_permissions(
    State(state): State<AppState>,
    Path(role): Path<String>,
    Json(req): Json<SetPermsReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if !["supervisor", "viewer"].contains(&role.as_str()) {
        return Err((StatusCode::BAD_REQUEST, Json(json!({
            "data": null,
            "error": { "code": "BAD_REQUEST", "message": "can only modify supervisor or viewer permissions" }
        }))));
    }

    auth_repo::set_role_permissions(&state.mssql, &role, req.paths)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({
            "data": null,
            "error": { "code": "DB_ERROR", "message": e.to_string() }
        }))))?;

    Ok(Json(json!({ "data": { "role": role }, "error": null })))
}
