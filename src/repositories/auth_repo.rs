use std::collections::HashMap;

use crate::{db::MssqlPool, errors::AppError};
use super::mssql_util::{exec, i32_val, str_val, opt_str, dt_str_or_empty};

// ─── Models ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub id:           i32,
    pub username:     String,
    pub password_hash:String,
    pub role:         String,
    pub display_name: String,
    pub created_at:   String,
}

#[derive(Debug)]
pub struct CreateUserReq {
    pub username:      String,
    pub display_name:  String,
    pub password_hash: String,
    pub role:          String,
}

#[derive(Debug)]
pub struct UpdateUserReq {
    pub display_name: String,
    pub role:         String,
}

// ─── User queries ─────────────────────────────────────────────────────────────

pub async fn find_by_username(pool: &MssqlPool, username: &str) -> Result<Option<UserRecord>, AppError> {
    let rows = exec(
        pool,
        "SELECT id, username, password_hash, role, display_name, created_at \
         FROM [dbo].[dashboard_users] WHERE username = @p1",
        &[username.to_string()],
    ).await?;

    Ok(rows.first().map(|r| UserRecord {
        id:            i32_val(r, "id"),
        username:      str_val(r, "username"),
        password_hash: str_val(r, "password_hash"),
        role:          str_val(r, "role"),
        display_name:  str_val(r, "display_name"),
        created_at:    dt_str_or_empty(r, "created_at"),
    }))
}

pub async fn list_users(pool: &MssqlPool) -> Result<Vec<UserRecord>, AppError> {
    let rows = exec(
        pool,
        "SELECT id, username, password_hash, role, display_name, created_at \
         FROM [dbo].[dashboard_users] ORDER BY id",
        &[],
    ).await?;

    Ok(rows.iter().map(|r| UserRecord {
        id:            i32_val(r, "id"),
        username:      str_val(r, "username"),
        password_hash: str_val(r, "password_hash"),
        role:          str_val(r, "role"),
        display_name:  str_val(r, "display_name"),
        created_at:    dt_str_or_empty(r, "created_at"),
    }).collect())
}

pub async fn create_user(pool: &MssqlPool, req: CreateUserReq) -> Result<UserRecord, AppError> {
    exec(
        pool,
        "INSERT INTO [dbo].[dashboard_users] (username, display_name, password_hash, role) \
         VALUES (@p1, @p2, @p3, @p4)",
        &[req.username.clone(), req.display_name.clone(), req.password_hash.clone(), req.role.clone()],
    ).await?;

    find_by_username(pool, &req.username).await?
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("Failed to fetch created user")))
}

pub async fn update_user(pool: &MssqlPool, id: i32, req: UpdateUserReq) -> Result<(), AppError> {
    exec(
        pool,
        "UPDATE [dbo].[dashboard_users] SET display_name=@p1, role=@p2 WHERE id=@p3",
        &[req.display_name, req.role, id.to_string()],
    ).await?;
    Ok(())
}

pub async fn set_password(pool: &MssqlPool, id: i32, hash: &str) -> Result<(), AppError> {
    exec(
        pool,
        "UPDATE [dbo].[dashboard_users] SET password_hash=@p1 WHERE id=@p2",
        &[hash.to_string(), id.to_string()],
    ).await?;
    Ok(())
}

pub async fn delete_user(pool: &MssqlPool, id: i32) -> Result<(), AppError> {
    exec(
        pool,
        "DELETE FROM [dbo].[dashboard_users] WHERE id=@p1",
        &[id.to_string()],
    ).await?;
    Ok(())
}

// ─── Permissions ─────────────────────────────────────────────────────────────

/// สร้าง table ถ้ายังไม่มี — เรียกตอน startup
pub async fn ensure_permissions_table(pool: &MssqlPool) -> Result<(), AppError> {
    exec(pool,
        "IF NOT EXISTS (SELECT 1 FROM INFORMATION_SCHEMA.TABLES \
                        WHERE TABLE_SCHEMA='dbo' AND TABLE_NAME='dashboard_role_permissions') \
         BEGIN \
           CREATE TABLE [dbo].[dashboard_role_permissions] ( \
             id   INT IDENTITY PRIMARY KEY, \
             role NVARCHAR(50)  NOT NULL, \
             path NVARCHAR(200) NOT NULL, \
             CONSTRAINT UQ_role_path UNIQUE (role, path) \
           ); \
           INSERT INTO [dbo].[dashboard_role_permissions] (role, path) VALUES \
             ('supervisor','/'),('supervisor','/live'),('supervisor','/inventory'), \
             ('supervisor','/wb-report'),('supervisor','/da-report'), \
             ('supervisor','/downtime'),('supervisor','/utilization'), \
             ('supervisor','/machine-detail'),('supervisor','/timeline'), \
             ('supervisor','/store-items'), \
             ('viewer','/'),('viewer','/live'),('viewer','/inventory'), \
             ('viewer','/wb-report'),('viewer','/da-report'); \
         END",
        &[],
    ).await?;
    Ok(())
}

/// คืน { "supervisor": [...], "viewer": [...] }
pub async fn get_permissions(pool: &MssqlPool) -> Result<HashMap<String, Vec<String>>, AppError> {
    let rows = exec(
        pool,
        "SELECT role, path FROM [dbo].[dashboard_role_permissions] ORDER BY role, path",
        &[],
    ).await?;

    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for r in &rows {
        map.entry(str_val(r, "role"))
           .or_default()
           .push(str_val(r, "path"));
    }
    Ok(map)
}

/// แทนที่สิทธิ์ทั้งหมดของ role นั้น
pub async fn set_role_permissions(pool: &MssqlPool, role: &str, paths: Vec<String>) -> Result<(), AppError> {
    // delete existing
    exec(pool,
        "DELETE FROM [dbo].[dashboard_role_permissions] WHERE role=@p1",
        &[role.to_string()],
    ).await?;

    // insert new paths
    for path in paths {
        exec(pool,
            "INSERT INTO [dbo].[dashboard_role_permissions] (role, path) VALUES (@p1, @p2)",
            &[role.to_string(), path],
        ).await?;
    }
    Ok(())
}

/// ดึง paths ที่ role นั้นเข้าได้
pub async fn get_role_paths(pool: &MssqlPool, role: &str) -> Result<Vec<String>, AppError> {
    let rows = exec(
        pool,
        "SELECT path FROM [dbo].[dashboard_role_permissions] WHERE role=@p1",
        &[role.to_string()],
    ).await?;
    Ok(rows.iter().map(|r| str_val(r, "path")).collect())
}
