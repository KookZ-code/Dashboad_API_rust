// repositories/item_repo.rs — SQL queries for items table
//
// กฎเหล็กของ repository layer:
// 1. มีแค่ SQL queries — ห้ามมี business logic
// 2. ห้าม SELECT * — ระบุ column ทุกครั้ง
// 3. ใช้ pool จาก parameter (ไม่สร้าง connection ใหม่)
// 4. Return Result — ไม่ unwrap() ใดๆ
// 5. ใช้ query_as! macro → compile-time type checking

use crate::db::DbPool;
use crate::models::{CreateItemRequest, Item, PaginationParams, UpdateItemRequest};
use chrono::Utc;

/// ดึงรายการ items แบบ pagination
/// ✅ ระบุ column ชัดเจน: id, name, description, created_at, updated_at
/// ✅ filter ด้วย deleted_at IS NULL (soft delete)
pub async fn find_all(
    pool: &DbPool,
    params: &PaginationParams,
) -> Result<(Vec<Item>, i64), sqlx::Error> {
    let offset = (params.page.saturating_sub(1)) as i64 * params.limit as i64;
    let limit = params.limit as i64;

    // Count query แยกออกมา ระบุ column ที่ count เสมอ
    let total: i64 = sqlx::query_scalar!(
        "SELECT COUNT(id) FROM items WHERE deleted_at IS NULL"
    )
    .fetch_one(pool)
    .await?;

    // Data query — ระบุทุก column ที่ต้องการ ห้าม SELECT *
    let items = sqlx::query_as!(
        Item,
        r#"
        SELECT
            id,
            name,
            description,
            created_at  AS "created_at: chrono::DateTime<chrono::Utc>",
            updated_at  AS "updated_at: chrono::DateTime<chrono::Utc>"
        FROM items
        WHERE deleted_at IS NULL
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
        limit,
        offset
    )
    .fetch_all(pool)
    .await?;

    Ok((items, total))
}

/// ดึง item เดี่ยวตาม ID
pub async fn find_by_id(pool: &DbPool, id: &str) -> Result<Option<Item>, sqlx::Error> {
    // ✅ ระบุทุก column
    // ✅ ใช้ parameterized query — ป้องกัน SQL injection อัตโนมัติ
    sqlx::query_as!(
        Item,
        r#"
        SELECT
            id,
            name,
            description,
            created_at  AS "created_at: chrono::DateTime<chrono::Utc>",
            updated_at  AS "updated_at: chrono::DateTime<chrono::Utc>"
        FROM items
        WHERE id = $1
          AND deleted_at IS NULL
        "#,
        id
    )
    .fetch_optional(pool)
    .await
}

/// สร้าง item ใหม่
pub async fn create(
    pool: &DbPool,
    id: &str,
    req: &CreateItemRequest,
) -> Result<Item, sqlx::Error> {
    let now = Utc::now();

    sqlx::query!(
        r#"
        INSERT INTO items (id, name, description, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        id,
        req.name,
        req.description,
        now,
        now,
    )
    .execute(pool)
    .await?;

    // Fetch กลับมา แทนที่จะ construct struct เอง
    // เพื่อให้ได้ข้อมูลที่ตรงกับ DB จริงๆ
    find_by_id(pool, id)
        .await?
        .ok_or(sqlx::Error::RowNotFound)
}

/// อัปเดต item (partial update — update เฉพาะ field ที่ส่งมา)
pub async fn update(
    pool: &DbPool,
    id: &str,
    req: &UpdateItemRequest,
) -> Result<Option<Item>, sqlx::Error> {
    let now = Utc::now();

    // Build dynamic UPDATE ตาม field ที่ส่งมา
    // ✅ ใช้ transaction เพื่อ atomic operation
    let mut tx = pool.begin().await?;

    let has_description = req.description.is_some();

    let rows_affected = sqlx::query!(
        r#"
        UPDATE items
        SET
            name        = COALESCE($1, name),
            description = CASE WHEN $2 THEN $3 ELSE description END,
            updated_at  = $4
        WHERE id = $5
          AND deleted_at IS NULL
        "#,
        req.name,
        has_description,
        req.description,
        now,
        id,
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;

    if rows_affected == 0 {
        return Ok(None);
    }

    find_by_id(pool, id).await
}

/// Soft delete — ไม่ลบจริง ใช้ deleted_at แทน
/// ✅ ไม่ใช้ DELETE statement — มี audit trail
pub async fn soft_delete(pool: &DbPool, id: &str) -> Result<bool, sqlx::Error> {
    let now = Utc::now();

    let rows_affected = sqlx::query!(
        r#"
        UPDATE items
        SET deleted_at = $1
        WHERE id = $2
          AND deleted_at IS NULL
        "#,
        now,
        id,
    )
    .execute(pool)
    .await?
    .rows_affected();

    Ok(rows_affected > 0)
}
