// handlers/items.rs — Items CRUD handlers
//
// กฎ: Handler ต้องบาง (thin layer)
// หน้าที่:
// 1. รับ request → extract + validate input
// 2. เรียก repository
// 3. แปลงเป็น response แล้ว return
// ห้ามมี SQL ที่นี่ → อยู่ใน repositories/ เท่านั้น

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};

use crate::{
    db::DbPool,
    errors::{AppError, AppResult},
    models::{
        ApiResponse, CreateItemRequest, ItemResponse, PaginatedResponse, PaginationParams,
        UpdateItemRequest, new_id,
    },
    repositories::item_repo,
};

/// GET /api/items?page=1&limit=20
pub async fn list_items(
    State(pool): State<DbPool>,
    Query(params): Query<PaginationParams>,
) -> AppResult<Json<ApiResponse<PaginatedResponse<ItemResponse>>>> {
    // Validate pagination params
    if params.limit > 100 {
        return Err(AppError::BadRequest("limit cannot exceed 100".to_string()));
    }

    let (items, total) = item_repo::find_all(&pool, &params).await?;

    let response = PaginatedResponse {
        data: items.into_iter().map(ItemResponse::from).collect(),
        page: params.page,
        limit: params.limit,
        total,
    };

    Ok(Json(ApiResponse::success(response)))
}

/// GET /api/items/:id
pub async fn get_item(
    State(pool): State<DbPool>,
    Path(id): Path<String>,
) -> AppResult<Json<ApiResponse<ItemResponse>>> {
    // Validate UUID format
    if id.is_empty() {
        return Err(AppError::BadRequest("id cannot be empty".to_string()));
    }

    let item = item_repo::find_by_id(&pool, &id)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(ApiResponse::success(ItemResponse::from(item))))
}

/// POST /api/items
pub async fn create_item(
    State(pool): State<DbPool>,
    Json(req): Json<CreateItemRequest>,
) -> AppResult<(StatusCode, Json<ApiResponse<ItemResponse>>)> {
    // Validate input
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::Validation("name cannot be empty".to_string()));
    }
    if name.len() > 255 {
        return Err(AppError::Validation(
            "name cannot exceed 255 characters".to_string(),
        ));
    }

    let id = new_id();
    let validated_req = CreateItemRequest {
        name,
        description: req.description.map(|d| d.trim().to_string()),
    };

    let item = item_repo::create(&pool, &id, &validated_req).await?;

    Ok((
        StatusCode::CREATED,
        Json(ApiResponse::success(ItemResponse::from(item))),
    ))
}

/// PUT /api/items/:id
pub async fn update_item(
    State(pool): State<DbPool>,
    Path(id): Path<String>,
    Json(req): Json<UpdateItemRequest>,
) -> AppResult<Json<ApiResponse<ItemResponse>>> {
    // อย่างน้อยต้องส่ง 1 field มาให้ update
    if req.name.is_none() && req.description.is_none() {
        return Err(AppError::BadRequest(
            "At least one field must be provided for update".to_string(),
        ));
    }

    // Validate name ถ้าส่งมา
    if let Some(ref name) = req.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Validation("name cannot be empty".to_string()));
        }
        if name.len() > 255 {
            return Err(AppError::Validation(
                "name cannot exceed 255 characters".to_string(),
            ));
        }
    }

    let item = item_repo::update(&pool, &id, &req)
        .await?
        .ok_or(AppError::NotFound)?;

    Ok(Json(ApiResponse::success(ItemResponse::from(item))))
}

/// DELETE /api/items/:id  (soft delete)
pub async fn delete_item(
    State(pool): State<DbPool>,
    Path(id): Path<String>,
) -> AppResult<StatusCode> {
    let deleted = item_repo::soft_delete(&pool, &id).await?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}
