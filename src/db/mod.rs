use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};
use tiberius::{AuthMethod, Config as TibConfig, EncryptionLevel};
use tracing::info;

use crate::config::Config;

// ─── SQLite (items CRUD) ──────────────────────────────────────────────────────

pub type DbPool = SqlitePool;

pub async fn create_sqlite_pool(database_url: &str) -> Result<DbPool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .idle_timeout(std::time::Duration::from_secs(600))
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(database_url)
        .await?;
    info!("SQLite pool created");
    Ok(pool)
}

pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await?;
    info!("All migrations applied successfully");
    Ok(())
}

// ─── MSSQL (dashboard API) ────────────────────────────────────────────────────

pub type MssqlPool = bb8::Pool<bb8_tiberius::ConnectionManager>;

/// สร้าง MSSQL connection pool ด้วย tiberius + bb8
pub async fn create_mssql_pool(cfg: &Config) -> anyhow::Result<MssqlPool> {
    let mut tib = TibConfig::new();
    tib.host(&cfg.db_server);
    tib.port(cfg.db_port);
    tib.database(&cfg.db_name);
    tib.authentication(AuthMethod::sql_server(&cfg.db_user, &cfg.db_password));
    // ปิด TLS — ใช้กับ internal SQL Server ที่ไม่ได้ตั้ง certificate
    tib.encryption(EncryptionLevel::NotSupported);

    let mgr = bb8_tiberius::ConnectionManager::build(tib)
        .map_err(|e| anyhow::anyhow!("MSSQL ConnectionManager error: {:?}", e))?;

    let pool = bb8::Pool::builder()
        .max_size(10)
        .connection_timeout(std::time::Duration::from_secs(30))
        .build(mgr)
        .await
        .map_err(|e| anyhow::anyhow!("MSSQL pool build error: {:?}", e))?;

    info!(
        server = %cfg.db_server,
        db = %cfg.db_name,
        "MSSQL pool created"
    );
    Ok(pool)
}
