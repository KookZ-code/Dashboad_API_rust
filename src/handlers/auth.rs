use std::time::Duration;

use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use crate::{
    handlers::AppState,
    middleware::jwt_auth::{Claims, sign_jwt},
    repositories::auth_repo::{self, CreateUserReq},
};

/// Password hash is never checked again once login goes through the Microchip
/// API — this placeholder just satisfies the NOT NULL column.
const MICROCHIP_MANAGED_PLACEHOLDER: &str = "microchip-managed";

#[derive(Deserialize)]
pub struct LoginReq {
    pub badgecode: String,
    pub password:  String,
}

#[derive(Deserialize)]
struct MicrochipResp {
    reply_code: i32,
    fullname:   Option<String>,
    imglink:    Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url:    Option<String>,
}

pub async fn login(
    State(state): State<AppState>,
    Json(req): Json<LoginReq>,
) -> Result<(StatusCode, Json<LoginResp>), (StatusCode, Json<serde_json::Value>)> {
    let err = || {
        let body = serde_json::json!({
            "data": null,
            "error": { "code": "UNAUTHORIZED", "message": "รหัสพนักงานหรือรหัสผ่านไม่ถูกต้อง" }
        });
        (StatusCode::UNAUTHORIZED, Json(body))
    };

    // ── Verify badge + password against the Microchip Account API ──────────
    // Note: this endpoint always responds HTTP 200, even on bad credentials —
    // success/failure is signaled by `reply_code` (1 = success, 0 = failure),
    // confirmed by a live test against the real API. Never branch on HTTP status.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(state.config.microchip_timeout_secs))
        .build()
        .map_err(|e| { tracing::error!("failed to build reqwest client: {e}"); err() })?;

    let resp = client
        .post(&state.config.microchip_api_url)
        .json(&serde_json::json!({ "badgecode": req.badgecode, "password": req.password }))
        .send()
        .await
        .map_err(|e| { tracing::warn!("microchip API unreachable: {e}"); err() })?;

    if !resp.status().is_success() {
        tracing::warn!("microchip API returned HTTP {}", resp.status());
        return Err(err());
    }

    let body: MicrochipResp = resp.json().await
        .map_err(|e| { tracing::warn!("microchip response unparseable: {e}"); err() })?;

    if body.reply_code != 1 {
        tracing::warn!("login failed for badge '{}': reply_code={}", req.badgecode, body.reply_code);
        return Err(err());
    }

    let fullname = body.fullname.unwrap_or_else(|| req.badgecode.clone());
    let image_url = body.imglink;

    // ── Resolve our own role mapping, keyed by badge code ───────────────────
    let user = match auth_repo::find_by_username(&state.mssql, &req.badgecode).await {
        Err(e) => { tracing::error!("DB error finding badge '{}': {:?}", req.badgecode, e); return Err(err()); }
        Ok(Some(u)) => {
            // Best-effort refresh of the cached display name — never fail login over this.
            if let Err(e) = auth_repo::touch_login(&state.mssql, &req.badgecode, &fullname).await {
                tracing::warn!("failed to refresh display_name for '{}': {:?}", req.badgecode, e);
            }
            u
        }
        Ok(None) => {
            // First time this badge has logged in — auto-create as viewer.
            match auth_repo::create_user(&state.mssql, CreateUserReq {
                username:      req.badgecode.clone(),
                display_name:  fullname.clone(),
                password_hash: MICROCHIP_MANAGED_PLACEHOLDER.to_string(),
                role:          "viewer".to_string(),
            }).await {
                Ok(u) => u,
                Err(e) => { tracing::error!("failed to auto-create badge '{}': {:?}", req.badgecode, e); return Err(err()); }
            }
        }
    };

    let exp = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::hours(state.config.jwt_expire_hours as i64))
        .expect("valid timestamp")
        .timestamp();

    let claims = Claims {
        sub:          req.badgecode.clone(),
        id:           user.id,
        display_name: fullname.clone(),
        role:         user.role.clone(),
        img:          image_url.clone(),
        exp,
    };

    let token = sign_jwt(&claims, &state.config.jwt_secret);

    Ok((StatusCode::OK, Json(LoginResp {
        data: LoginData {
            token,
            user: UserInfo {
                id:           user.id,
                username:     req.badgecode,
                display_name: fullname,
                role:         user.role,
                image_url,
            },
        },
        error: None,
    })))
}
