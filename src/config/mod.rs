use std::env;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct Config {
    // SQLite (items CRUD)
    pub database_url: String,
    // Server
    pub port: u16,
    pub frontend_origin: String,
    pub environment: Environment,
    // MSSQL (dashboard API)
    pub db_server: String,
    pub db_port: u16,
    pub db_name: String,
    pub db_user: String,
    pub db_password: String,
    // Dashboard API config
    /// ถ้าว่าง = ปิด auth
    pub api_key: String,
    pub view_name: String,
    pub machine_table: String,
    // WB-UPH module — SQLite `central.db` (hourly bond-unit scan records)
    pub central_db_path: String,
    // DA-UPH module — PostgreSQL `uph` database (Die Attach scan output).
    /// ว่าง = ปิด DA-UPH module (endpoints คืน 503)
    pub da_db_url: String,
    // Oracle (ISO/FS area job/downtime data). All optional; ora_enabled gates usage.
    pub ora_enabled: bool,
    pub ora_user: String,
    pub ora_password: String,
    pub ora_dsn: String,
    pub ora_client_lib: String,
    pub ora_view: String,
    pub ora_live_view: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Environment {
    Development,
    Production,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    MissingVar(String),
    #[error("Invalid value for {key}: {reason}")]
    InvalidValue { key: String, reason: String },
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: require_env("DATABASE_URL")?,
            port: parse_env("PORT", 8090)?,
            frontend_origin: require_env("FRONTEND_ORIGIN")?,
            environment: parse_environment()?,
            db_server: require_env("DB_SERVER")?,
            db_port: parse_env("DB_PORT", 1433)?,
            db_name: require_env("DB_NAME")?,
            db_user: require_env("DB_USER")?,
            db_password: require_env("DB_PASSWORD")?,
            api_key: env::var("API_KEY").unwrap_or_default(),
            view_name: env::var("VIEW_NAME").unwrap_or_else(|_| "vw_job_nokey".to_string()),
            machine_table: env::var("MACHINE_TABLE").unwrap_or_else(|_| "dbo.machine".to_string()),
            central_db_path: require_env("CENTRAL_DB_PATH")?,
            da_db_url: env::var("DA_DB_URL").unwrap_or_default(),
            ora_enabled: env::var("ORA_ENABLED").unwrap_or_default() == "1",
            ora_user: env::var("ORA_USER").unwrap_or_default(),
            ora_password: env::var("ORA_PASSWORD").unwrap_or_default(),
            ora_dsn: env::var("ORA_DSN").unwrap_or_default(),
            ora_client_lib: env::var("ORA_CLIENT_LIB").unwrap_or_default(),
            ora_view: env::var("ORA_VIEW").unwrap_or_else(|_| "Vw_Asodowntime_2025on".to_string()),
            ora_live_view: env::var("ORA_LIVE_VIEW").unwrap_or_else(|_| "EQ_USER.V_EQDOWNTIME".to_string()),
        })
    }

    pub fn is_production(&self) -> bool {
        self.environment == Environment::Production
    }
}

fn require_env(key: &str) -> Result<String, ConfigError> {
    env::var(key).map_err(|_| ConfigError::MissingVar(key.to_string()))
}

fn parse_env<T>(key: &str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr + std::fmt::Display,
    T::Err: std::fmt::Display,
{
    match env::var(key) {
        Ok(val) => val.parse::<T>().map_err(|e| ConfigError::InvalidValue {
            key: key.to_string(),
            reason: e.to_string(),
        }),
        Err(_) => Ok(default),
    }
}

fn parse_environment() -> Result<Environment, ConfigError> {
    let env_str = env::var("ENVIRONMENT").unwrap_or_else(|_| "development".to_string());
    match env_str.to_lowercase().as_str() {
        "production" | "prod" => Ok(Environment::Production),
        "development" | "dev" => Ok(Environment::Development),
        other => Err(ConfigError::InvalidValue {
            key: "ENVIRONMENT".to_string(),
            reason: format!("expected 'production' or 'development', got '{}'", other),
        }),
    }
}
