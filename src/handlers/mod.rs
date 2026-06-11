pub mod da;
pub mod da_uph;
pub mod docs;
pub mod downtime;
pub mod health;
pub mod inventory;
pub mod items;
pub mod master;
pub mod overview;
pub mod tech;
pub mod utilization;
pub mod wb;
pub mod wb_uph;

use std::sync::Arc;
use axum::extract::FromRef;
use crate::{config::Config, db::{DbPool, MssqlPool, PgPool}, oracle::OracleCache};

#[derive(Clone)]
pub struct AppState {
    pub sqlite: DbPool,
    pub mssql:  MssqlPool,
    pub config: Config,
    pub oracle: Arc<OracleCache>,
    /// DA-UPH Postgres pool — None when DA_DB_URL is unset or unreachable at startup,
    /// so a DA-workstation outage degrades only da-uph/* (not the whole API center).
    pub pg: Option<PgPool>,
}

impl AppState {
    pub fn new(sqlite: DbPool, mssql: MssqlPool, oracle: Arc<OracleCache>, config: Config, pg: Option<PgPool>) -> Self {
        Self { sqlite, mssql, config, oracle, pg }
    }
}

impl FromRef<AppState> for DbPool {
    fn from_ref(state: &AppState) -> Self { state.sqlite.clone() }
}

impl FromRef<AppState> for MssqlPool {
    fn from_ref(state: &AppState) -> Self { state.mssql.clone() }
}

impl FromRef<AppState> for Config {
    fn from_ref(state: &AppState) -> Self { state.config.clone() }
}
