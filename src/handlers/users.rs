use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::task::spawn_blocking;

use crate::{
    errors::AppResult,
    handlers::AppState,
    repositories::auth_repo::{self, CreateUserReq, UpdateUserReq},
};

#[derive(Serialize)]
struct UserRow {
    id:           i32,
    username:     String,
    display_name: String,
    role:         String,
    created_at:   String,
}

fn to_row(u: auth_repo::UserRecord) -> UserRow {
    UserRow { id: u.id, username: u.username, display_name: u.display_name, role: u.role, created_at: u.created_at }
}

pub async fn list_users(State(state): State<AppState>) -> Result<Json<Value>, Json<Value>> {
    let users = auth_repo::list_users(&state.mssql)
        .await
        .map_err(|e| json!({ "data": null, "error": { "code": "DB_ERROR", "message": e.to_string() } }))?;
    Ok(Json(json!({ "data": users.into_iter().map(to_row).collect::<Vec<_>>(), "error": null })))
}

#[derive(Deserialize)]
pub struct CreateReq {
    username:     String,
    display_name: String,
    password:     String,
    role:         String,
}

pub async fn create_user(
    State(state): State<AppState>,
    Json(req): Json<CreateReq>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let err = |msg: &str| (StatusCode::BAD_REQUEST, Json(json!({ "data": null, "error": { "code": "BAD_REQUEST", "message": msg } })));

    if req.username.trim().is_empty() { return Err(err("username is required")); }
    if req.password.len() < 6 { return Err(err("password must be at least 6 characters")); }
    if !["admin","supervisor","viewer"].contains(&req.role.as_str()) { return Err(err("invalid role")); }

    let pass = req.password.clone();
    let hash = spawn_blocking(move || bcrypt::hash(pass, bcrypt::DEFAULT_COST))
        .await
        .map_err(|_| err("hashing failed"))?
        .map_err(|_| err("hashing failed"))?;

    let user = auth_repo::create_user(&state.mssql, CreateUserReq {
        username: req.username,
        display_name: req.display_name,
        password_hash: hash,
        role: req.role,
    }).await.map_err(|e| err(&e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({ "data": to_row(user), "error": null }))))
}

#[derive(Deserialize)]
pub struct UpdateReq {
    display_name: String,
    role:         String,
}

pub async fn update_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(req): Json<UpdateReq>,
) -> Result<Json<Value>, Json<Value>> {
    let err = |msg: &str| Json(json!({ "data": null, "error": { "code": "BAD_REQUEST", "message": msg } }));
    if !["admin","supervisor","viewer"].contains(&req.role.as_str()) { return Err(err("invalid role")); }

    auth_repo::update_user(&state.mssql, id, UpdateUserReq {
        display_name: req.display_name,
        role: req.role,
    }).await.map_err(|e| err(&e.to_string()))?;

    Ok(Json(json!({ "data": { "id": id }, "error": null })))
}

#[derive(Deserialize)]
pub struct PasswordReq {
    password: String,
}

pub async fn set_password(
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(req): Json<PasswordReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let err = |msg: &str| (StatusCode::BAD_REQUEST, Json(json!({ "data": null, "error": { "code": "BAD_REQUEST", "message": msg } })));
    if req.password.len() < 6 { return Err(err("password must be at least 6 characters")); }

    let pass = req.password.clone();
    let hash = spawn_blocking(move || bcrypt::hash(pass, bcrypt::DEFAULT_COST))
        .await.map_err(|_| err("hashing failed"))?
        .map_err(|_| err("hashing failed"))?;

    auth_repo::set_password(&state.mssql, id, &hash)
        .await.map_err(|e| err(&e.to_string()))?;

    Ok(Json(json!({ "data": { "id": id }, "error": null })))
}

pub async fn delete_user(
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, Json<Value>> {
    auth_repo::delete_user(&state.mssql, id)
        .await.map_err(|e| Json(json!({ "data": null, "error": { "code": "DB_ERROR", "message": e.to_string() } })))?;
    Ok(Json(json!({ "data": { "deleted": id }, "error": null })))
}
