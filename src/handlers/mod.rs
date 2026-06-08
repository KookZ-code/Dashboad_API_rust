pub mod da;
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

use axum::extract::FromRef;
use crate::{config::Config, db::{DbPool, MssqlPool}};

#[derive(Clone)]
pub struct AppState {
    pub sqlite: DbPool,
    pub mssql:  MssqlPool,
    pub config: Config,
}

impl AppState {
    pub fn new(sqlite: DbPool, mssql: MssqlPool, config: Config) -> Self {
        Self { sqlite, mssql, config }
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
