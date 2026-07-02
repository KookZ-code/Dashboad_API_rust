use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(serde::Serialize)]
pub struct ErrBody { pub data: Option<()>, pub error: ErrDetail }
#[derive(serde::Serialize)]
pub struct ErrDetail { pub code: &'static str, pub message: &'static str }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub:          String,   // username
    pub id:           i32,
    pub display_name: String,
    pub role:         String,
    pub exp:          i64,      // unix timestamp
}

// ─── Pure-Rust HS256 JWT ──────────────────────────────────────────────────────

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

fn b64_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn b64_decode(s: &str) -> Result<Vec<u8>, ()> {
    URL_SAFE_NO_PAD.decode(s).map_err(|_| ())
}

pub fn sign_jwt(claims: &Claims, secret: &str) -> String {
    let header  = b64_encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = b64_encode(serde_json::to_string(claims).unwrap().as_bytes());
    let msg     = format!("{}.{}", header, payload);

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(msg.as_bytes());
    let sig = b64_encode(&mac.finalize().into_bytes());

    format!("{}.{}", msg, sig)
}

pub fn verify_jwt(token: &str, secret: &str) -> Option<Claims> {
    let parts: Vec<&str> = token.splitn(3, '.').collect();
    if parts.len() != 3 { return None; }

    let msg = format!("{}.{}", parts[0], parts[1]);
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(msg.as_bytes());
    let expected = b64_encode(&mac.finalize().into_bytes());
    if expected != parts[2] { return None; }

    let payload_bytes = b64_decode(parts[1]).ok()?;
    let claims: Claims = serde_json::from_slice(&payload_bytes).ok()?;

    let now = chrono::Utc::now().timestamp();
    if claims.exp < now { return None; }

    Some(claims)
}

// ─── Axum middleware ──────────────────────────────────────────────────────────

pub async fn require_jwt(
    State(config): State<Config>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, (StatusCode, Json<ErrBody>)> {
    let token = req.headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if token.is_empty() {
        return Err((StatusCode::UNAUTHORIZED, Json(ErrBody {
            data: None,
            error: ErrDetail { code: "UNAUTHORIZED", message: "Missing token" },
        })));
    }

    match verify_jwt(token, &config.jwt_secret) {
        Some(claims) => {
            req.extensions_mut().insert(claims);
            Ok(next.run(req).await)
        }
        None => Err((StatusCode::UNAUTHORIZED, Json(ErrBody {
            data: None,
            error: ErrDetail { code: "UNAUTHORIZED", message: "Invalid or expired token" },
        }))),
    }
}
