mod app;
mod config;
mod db;
mod errors;
mod handlers;
mod helpers;
mod middleware;
mod models;
mod oracle;
pub mod repositories;

use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    dotenvy::dotenv().ok();

    let config = config::Config::from_env()?;
    info!("Configuration loaded: port={}", config.port);

    // SQLite pool สำหรับ items CRUD
    let sqlite = db::create_sqlite_pool(&config.database_url).await?;
    db::run_migrations(&sqlite).await?;
    info!("SQLite ready");

    // MSSQL pool สำหรับ dashboard API
    let mssql = db::create_mssql_pool(&config).await
        .map_err(|e| anyhow::anyhow!("MSSQL pool error: {:?}", e))?;
    info!("MSSQL pool ready");

    // สร้าง dashboard_role_permissions table ถ้ายังไม่มี
    repositories::auth_repo::ensure_permissions_table(&mssql).await
        .map_err(|e| anyhow::anyhow!("Failed to init permissions table: {:?}", e))?;
    info!("Auth tables ready");

    // Pre-warm central.db mirror (WB-UPH) — copy from network share in background so it's
    // ready before the first request instead of blocking that first request.
    let central_path = config.central_db_path.clone();
    tokio::task::spawn_blocking(move || {
        repositories::wb_uph_repo::warmup(&central_path);
    });

    // Postgres pool สำหรับ DA-UPH — optional. ถ้า DA_DB_URL ว่างหรือต่อไม่ติดตอน startup
    // ให้ degrade เป็น None (da-uph/* คืน 503) แทนที่จะล้มทั้ง server.
    let pg = if config.da_db_url.is_empty() {
        info!("DA-UPH disabled (DA_DB_URL unset)");
        None
    } else {
        match db::create_pg_pool(&config.da_db_url).await {
            Ok(p) => { info!("Postgres (DA-UPH) pool ready"); Some(p) }
            Err(e) => { tracing::warn!("DA-UPH Postgres unavailable (da-uph/* will 503): {e}"); None }
        }
    };

    // Oracle cache (ISO/FS) — load in background, refresh on a timer (off if ORA_ENABLED!=1)
    let oracle = std::sync::Arc::new(oracle::OracleCache::from_config(&config));
    if config.ora_enabled {
        let hist = oracle.clone();
        tokio::spawn(async move {
            loop {
                let c = hist.clone();
                let _ = tokio::task::spawn_blocking(move || c.refresh_historical()).await;
                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
            }
        });
        let live = oracle.clone();
        tokio::spawn(async move {
            loop {
                let c = live.clone();
                let _ = tokio::task::spawn_blocking(move || c.refresh_live()).await;
                tokio::time::sleep(std::time::Duration::from_secs(300)).await;
            }
        });
        info!("Oracle refresh tasks started (ISO/FS)");
    } else {
        info!("Oracle disabled (ORA_ENABLED != 1) — ISO/FS served MSSQL-only");
    }

    let app = app::create_app(sqlite, mssql, oracle, config.clone(), pg);

    let addr = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Server listening on http://{}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Server shut down gracefully");
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to install Ctrl+C handler");
    tracing::warn!("Shutdown signal received");
}
