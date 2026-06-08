use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use serde::Serialize;

use crate::config::Config;

#[derive(Serialize)]
pub struct ErrBody {
    data: Option<()>,
    error: ErrDetail,
}

#[derive(Serialize)]
pub struct ErrDetail {
    code: &'static str,
    message: &'static str,
}

/// ตรวจ `x-api-key` header ถ้า `config.api_key` ไม่ว่าง
/// ถ้าว่าง = ปิด auth (dev mode)
pub async fn require_api_key(
    State(config): State<Config>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrBody>)> {
    if config.api_key.is_empty() {
        return Ok(next.run(req).await);
    }

    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided != config.api_key {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrBody {
                data: None,
                error: ErrDetail { code: "UNAUTHORIZED", message: "Invalid API key" },
            }),
        ));
    }

    Ok(next.run(req).await)
}
