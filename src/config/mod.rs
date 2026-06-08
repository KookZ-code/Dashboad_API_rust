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
            port: parse_env("PORT", 8080)?,
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
