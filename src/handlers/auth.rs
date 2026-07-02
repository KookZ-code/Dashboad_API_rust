use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;

use crate::{
    handlers::AppState,
    middleware::jwt_auth::{Claims, sign_jwt},
    repositories::auth_repo,
};

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResp {
    pub data:  LoginData,
    pub error: Option<()>,
}

#[derive(Serialize)]
pub struct LoginData {
    pub token: String,
    pub user:  UserInfo,
}

#[derive(Serialize)]
pub struct UserInfo {
    pub id:           i32,
    pub username:     String,
    pub display_name: String,
    pub role:         String,
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginReq>,
) -> Result<(StatusCode, Json<LoginResp>), (StatusCode, Json<serde_json::Value>)> {
    let err = |msg: &str| {
        let body = serde_json::json!({ "data": null, "error": { "code": "UNAUTHORIZED", "message": msg } });
        (StatusCode::UNAUTHORIZED, Json(body))
    };

    let user = match auth_repo::find_by_username(&state.mssql, &req.username).await {
        Err(e) => { tracing::error!("DB error finding user '{}': {:?}", req.username, e); return Err(err("Login failed")); }
        Ok(None) => { tracing::warn!("User not found: '{}'", req.username); return Err(err("Invalid username or password")); }
        Ok(Some(u)) => { tracing::info!("Found user '{}' role='{}' hash_prefix='{}'", u.username, u.role, &u.password_hash[..10]); u }
    };

    // bcrypt verify (blocking)
    let hash = user.password_hash.clone();
    let password = req.password.clone();
    let valid = spawn_blocking(move || bcrypt::verify(&password, &hash))
        .await
        .map_err(|_| err("Login failed"))?
        .unwrap_or(false);

    tracing::info!("bcrypt verify result: {}", valid);
    if !valid {
        return Err(err("Invalid username or password"));
    }

    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(state.config.jwt_expire_hours as i64))
        .expect("valid timestamp")
        .timestamp();

    let claims = Claims {
        sub:          user.username.clone(),
        id:           user.id,
        display_name: user.display_name.clone(),
        role:         user.role.clone(),
        exp,
    };

    let token = sign_jwt(&claims, &state.config.jwt_secret);

    Ok((StatusCode::OK, Json(LoginResp {
        data: LoginData {
            token,
            user: UserInfo {
                id:           user.id,
                username:     user.username,
                display_name: user.display_name,
                role:         user.role,
            },
        },
        error: None,
    })))
}
