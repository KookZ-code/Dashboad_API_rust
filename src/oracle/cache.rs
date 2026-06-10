// In-memory Oracle cache for ISO/FS data. The Oracle views are slow (~20s), so we
// load the full dataset once and refresh on a timer (historical 10 min, live 5 min),
// then filter/aggregate in memory per request — a direct port of oracle_db.py.
//
// rusqlite-style: the `oracle` crate is synchronous → callers refresh via spawn_blocking.

use anyhow::{anyhow, Result};
use chrono::{NaiveDate, NaiveDateTime};
use std::sync::RwLock;

use crate::config::Config;
use super::model::{area_from_equipment_type, OracleLiveRow, OracleRow};

const SETUP_KW: &[&str] = &["SETUP", "SET UP", "SET D/V", "CHANGE"];
const PM_KW: &[&str] = &["PM", "PREVENTIVE"];

pub struct OracleCache {
    pub enabled: bool,
    user: String,
    password: String,
    dsn: String,
    view: String,
    live_view: String,
    hist: RwLock<Vec<OracleRow>>,
    live: RwLock<Vec<OracleLiveRow>>,
}

/// Get a nullable string column, trimmed, default "".
fn sget(row: &oracle::Row, idx: usize) -> String {
    row.get::<usize, Option<String>>(idx)
        .ok()
        .flatten()
        .unwrap_or_default()
        .trim()
        .to_string()
}

impl OracleCache {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            enabled: cfg.ora_enabled,
            user: cfg.ora_user.clone(),
            password: cfg.ora_password.clone(),
            dsn: cfg.ora_dsn.clone(),
            view: cfg.ora_view.clone(),
            live_view: cfg.ora_live_view.clone(),
            hist: RwLock::new(Vec::new()),
            live: RwLock::new(Vec::new()),
        }
    }

    fn connect(&self) -> Result<oracle::Connection> {
        oracle::Connection::connect(&self.user, &self.password, &self.dsn)
            .map_err(|e| anyhow!("oracle connect failed: {e}"))
    }

    // ── Loads (blocking — call via spawn_blocking) ──────────────────────────

    fn fetch_historical(&self) -> Result<Vec<OracleRow>> {
        let conn = self.connect()?;
        let sql = format!(
            "SELECT EQUIPMENT_TYPE, EQUIPMENT_ID, S_DATE, P_START, P_STOP, JOB_TYPE, CAUSE, \
                    CRITERIA, DOWNTIME, WAIT_TECH, BADGE_NO, SHIFT, PKG, LOT_ID, PRODUCT_ID, \
                    TECHNICIAN_COMMENT \
             FROM {} \
             WHERE P_START IS NOT NULL AND P_STOP IS NOT NULL AND P_STOP > P_START",
            self.view
        );
        let mut out = Vec::new();
        let rows = conn.query(&sql, &[])?;
        for row in rows {
            let row = row?;
            let et = sget(&row, 0);
            let area = match area_from_equipment_type(&et) {
                Some(a) => a.to_string(),
                None => continue, // skip non-ISO/FS (e.g. ROBOT)
            };
            out.push(OracleRow {
                machine_id: sget(&row, 1),
                datex: row.get::<usize, Option<NaiveDateTime>>(2)?,
                date_ack: row.get::<usize, Option<NaiveDateTime>>(3)?,
                date_close: row.get::<usize, Option<NaiveDateTime>>(4)?,
                job_type: sget(&row, 5),
                cause: sget(&row, 6),
                symptom: sget(&row, 7),
                repair_min: row.get::<usize, Option<f64>>(8)?.unwrap_or(0.0),
                wait_min: row.get::<usize, Option<f64>>(9)?.unwrap_or(0.0),
                badge: sget(&row, 10),
                shift_code: sget(&row, 11),
                package_type: sget(&row, 12),
                lot_no: sget(&row, 13),
                die_mask: sget(&row, 14),
                action: sget(&row, 15),
                area,
            });
        }
        Ok(out)
    }

    fn fetch_live(&self) -> Result<Vec<OracleLiveRow>> {
        let conn = self.connect()?;
        let sql = format!(
            "SELECT EQUIPMENT_TYPE, EQUIPMENT_ID, CAUSE, CRITERIA, P_START, P_STOP, STATUS, \
                    BADGE_NO, NAME, NVL(WAIT_TECH,0), NVL(DOWNTIME,0), S_DATE, PKG, LOT_ID, \
                    PRODUCT_ID, TECHNICIAN_COMMENT \
             FROM {} \
             WHERE EQUIPMENT_TYPE IN ('ISOLATE','FORM_SING') AND S_DATE >= TRUNC(SYSDATE) - 1 \
             ORDER BY P_START DESC",
            self.live_view
        );
        let mut out = Vec::new();
        let rows = conn.query(&sql, &[])?;
        for row in rows {
            let row = row?;
            let et = sget(&row, 0);
            let area = match area_from_equipment_type(&et) {
                Some(a) => a.to_string(),
                None => continue,
            };
            let cause = sget(&row, 2);
            let criteria = sget(&row, 3);
            let p_start = row.get::<usize, Option<NaiveDateTime>>(4)?;
            let p_stop = row.get::<usize, Option<NaiveDateTime>>(5)?;
            let status_raw = sget(&row, 6);

            // job_type derivation (port of fetch_oracle_live_status)
            let cu = criteria.to_uppercase();
            let ca = cause.to_uppercase();
            let job_type = if SETUP_KW.iter().any(|k| cu.contains(k)) {
                "SETUP"
            } else if PM_KW.iter().any(|k| ca.contains(k)) {
                "PM"
            } else {
                "M/C DOWN"
            };

            // status mapping
            let mut status = match status_raw.as_str() {
                "Working" | "Waiting Approval" => "On Process",
                "Completed" => "Closed",
                _ => "Waiting",
            };
            if p_stop.is_none() && p_start.is_some() {
                status = "On Process";
            }

            out.push(OracleLiveRow {
                machine_id: sget(&row, 1),
                area,
                job_type: job_type.to_string(),
                des_job: criteria,
                status: status.to_string(),
                date_close: p_stop,
                wait_min: row.get::<usize, Option<f64>>(9)?.unwrap_or(0.0) as i64,
                repair_min: row.get::<usize, Option<f64>>(10)?.unwrap_or(0.0) as i64,
                badge: sget(&row, 7),
                package_type: sget(&row, 12),
                lot_no: sget(&row, 13),
                die_mask: sget(&row, 14),
            });
        }
        Ok(out)
    }

    /// Refresh the historical cache; keep the old data on failure (serve-stale).
    pub fn refresh_historical(&self) {
        match self.fetch_historical() {
            Ok(v) => {
                let n = v.len();
                *self.hist.write().unwrap() = v;
                tracing::info!("Oracle historical loaded: {n} rows");
            }
            Err(e) => tracing::warn!("Oracle historical load failed: {e}"),
        }
    }

    pub fn refresh_live(&self) {
        match self.fetch_live() {
            Ok(v) => {
                let n = v.len();
                *self.live.write().unwrap() = v;
                tracing::info!("Oracle live loaded: {n} rows");
            }
            Err(e) => tracing::warn!("Oracle live load failed: {e}"),
        }
    }

    // ── In-memory filtered reads ────────────────────────────────────────────

    /// Historical rows filtered by area (∩ ISO/FS), date range, and shift.
    /// `areas` = the request's area list (None = all). Returns only ISO/FS rows.
    pub fn filter_historical(
        &self,
        areas: Option<&[String]>,
        start: Option<&str>,
        end: Option<&str>,
        shift: Option<&str>,
    ) -> Vec<OracleRow> {
        let want: Option<Vec<String>> = areas.map(|a| {
            a.iter()
                .filter(|x| x.as_str() == "ISO" || x.as_str() == "FS")
                .cloned()
                .collect()
        });
        // If the caller filtered to areas with no ISO/FS, return nothing.
        if matches!(&want, Some(w) if w.is_empty()) {
            return Vec::new();
        }
        let start_d = start.and_then(parse_date);
        let end_d = end.and_then(parse_date).map(|d| d.succ_opt().unwrap_or(d)); // exclusive +1 day
        let shift_code = match shift.map(|s| s.to_uppercase()) {
            Some(ref s) if s == "DAY" || s == "D" => Some("D"),
            Some(ref s) if s == "NIGHT" || s == "N" => Some("N"),
            _ => None,
        };

        self.hist
            .read()
            .unwrap()
            .iter()
            .filter(|r| match &want {
                Some(w) => w.iter().any(|a| a == &r.area),
                None => true, // all ISO/FS (cache only holds ISO/FS)
            })
            .filter(|r| match (start_d, r.datex) {
                (Some(s), Some(dt)) => dt.date() >= s,
                (Some(_), None) => false,
                _ => true,
            })
            .filter(|r| match (end_d, r.datex) {
                (Some(e), Some(dt)) => dt.date() < e,
                (Some(_), None) => false,
                _ => true,
            })
            .filter(|r| match shift_code {
                Some(sc) => r.shift_code == sc,
                None => true,
            })
            .cloned()
            .collect()
    }

    /// Live rows filtered to the requested ISO/FS areas (None = all ISO/FS).
    pub fn live_filtered(&self, areas: Option<&[String]>) -> Vec<OracleLiveRow> {
        let want: Option<Vec<String>> = areas.map(|a| {
            a.iter()
                .filter(|x| x.as_str() == "ISO" || x.as_str() == "FS")
                .cloned()
                .collect()
        });
        if matches!(&want, Some(w) if w.is_empty()) {
            return Vec::new();
        }
        self.live
            .read()
            .unwrap()
            .iter()
            .filter(|r| match &want {
                Some(w) => w.iter().any(|a| a == &r.area),
                None => true,
            })
            .cloned()
            .collect()
    }
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(&s[..s.len().min(10)], "%Y-%m-%d").ok()
}
