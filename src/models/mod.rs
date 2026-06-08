// models/mod.rs — Data models
//
// กฎ:
// 1. Struct เหล่านี้ map 1:1 กับ column ใน database
// 2. ห้ามมี method ที่ยุ่งกับ DB connection
// 3. ระบุ column ที่ต้องการจริงๆ ใน struct (ห้าม SELECT *)
// 4. แยก "DB model" กับ "API response model" ออกจากกัน

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────────
// DB Models — map กับ rows ที่ได้จาก database query
// ─────────────────────────────────────────────────────────────────

/// Map กับ table: items
/// Column ที่ select ต้องตรงกับ field ใน struct ทุกตัว
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Item {
    pub id: String,           // UUID stored as TEXT ใน SQLite
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // deleted_at ไม่อยู่ใน struct นี้ — soft delete ทำใน WHERE clause
}

// ─────────────────────────────────────────────────────────────────
// API Request Models — รับ input จาก HTTP request body
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateItemRequest {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateItemRequest {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_page() -> u32 { 1 }
fn default_limit() -> u32 { 20 }

// ─────────────────────────────────────────────────────────────────
// API Response Models — ส่งออกไปยัง client
// แยกจาก DB model เพื่อ control ว่าจะเปิดเผย field อะไร
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ItemResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Item> for ItemResponse {
    fn from(item: Item) -> Self {
        Self {
            id: item.id,
            name: item.name,
            description: item.description,
            created_at: item.created_at,
            updated_at: item.updated_at,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    pub data: Vec<T>,
    pub page: u32,
    pub limit: u32,
    pub total: i64,
}

/// Standard API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub data: Option<T>,
    pub error: Option<()>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            data: Some(data),
            error: None,
        }
    }
}

/// Generate new UUID สำหรับ primary key
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}
