use chrono::NaiveDateTime;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

use crate::{db::MssqlPool, errors::AppError};
use super::mssql_util::*;

const LOST_TYPES: &[&str] = &[
    "SETUP", "SETUP BY OPERATOR", "CONVERT", "CLEAN MOLD",
    "CHANGE CAP", "FACILITY DOWN", "ENGINEERING DOWN",
];
const SETUP_CONV: &[&str] = &["SETUP", "CONVERT", "CLEAN MOLD", "CHANGE CAP"];
const SHIFT_MIN: f64 = 720.0;
const TECH_JT: &[&str] = &["M/C DOWN", "ENGINEERING DOWN", "FACILITY DOWN", "SETUP", "CONVERT"];

pub struct WbRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub view:          String,
    pub machine_table: String,
}

impl<'a> WbRepo<'a> {
    pub fn new(pool: &'a MssqlPool, view: impl Into<String>, machine_table: impl Into<String>) -> Self {
        Self { pool, view: view.into(), machine_table: machine_table.into() }
    }

    pub fn shift_window(date: &str, shift: &str) -> (String, String, String) {
        if shift == "Day" {
            return (format!("{} 07:00:00", date), format!("{} 19:00:00", date), "07:00 → 19:00".into());
        }
        // Night: prev-evening 19:00 → this-morning 07:00
        let prev = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .map(|d| d.pred_opt().unwrap_or(d))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|_| date.to_string());
        (format!("{} 19:00:00", prev), format!("{} 07:00:00", date), "19:00 → 07:00".into())
    }

    pub async fn packages(&self, date: &str) -> Result<Value, AppError> {
        let sql = format!(
            "SELECT DISTINCT [Package Type] AS package_type FROM {} \
             WHERE [id_operation] = 'WB' AND [Package Type] IS NOT NULL AND [Package Type] != '' \
               AND [datex] >= DATEADD(DAY, -7, @p1) AND [datex] < DATEADD(DAY, 1, @p1) \
             ORDER BY [Package Type]",
            self.view
        );
        let rows = exec(self.pool, &sql, &[date.to_string()]).await?;
        let pkgs: Vec<String> = rows.iter()
            .map(|r| str_val(r, "package_type"))
            .filter(|s| !s.is_empty())
            .collect();

        let mut opts: Vec<Value> = vec![json!({ "value": "__ALL__", "label": "— All Packages —" })];
        if pkgs.iter().any(|p| p.to_uppercase().contains("QFN")) {
            opts.push(json!({ "value": "__QFN__", "label": "— All QFN —" }));
        }
        for p in &pkgs {
            opts.push(json!({ "value": p, "label": p }));
        }
        Ok(json!({ "options": opts, "packages": pkgs }))
    }

    pub async fn report(&self, date: &str, shift: &str, packages: &str) -> Result<Value, AppError> {
        let (shift_start, shift_end, time_range) = Self::shift_window(date, shift);
        let params_ev = vec![shift_start.clone(), shift_end.clone(), date.to_string()];
        let params_open = vec![];

        let sql_ev = format!(
            "WITH key_mc AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine FROM {} \
               WHERE [id_operation] = 'WB' AND [flag_key] = 1 AND ISNULL([flag_delete],0) != 1 \
             ), shift_ev AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine, [job_type], [datex], [date_ack], \
                      [date_close], [des_job], ISNULL([Waiting_time],0) AS wait_min, [Package Type] AS package_type, \
                      NULLIF(RTRIM(LTRIM(ISNULL([by_perform],[by_ack]))),'') AS tech_name \
               FROM {} WHERE [id_operation]='WB' AND [datex]>=@p1 AND [datex]<@p2 \
                 AND [date_close] IS NOT NULL AND [date_close]>[datex] \
             ), pkg_scan AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine, [Package Type] AS package_type, \
                      ROW_NUMBER() OVER (PARTITION BY RTRIM(LTRIM([code_machine])) ORDER BY [datex] DESC) AS rn \
               FROM {} WHERE [id_operation]='WB' AND [Package Type] IS NOT NULL AND [Package Type]!='' \
                 AND [datex]>=DATEADD(DAY,-7,CAST(@p3 AS DATE)) AND [datex]<DATEADD(DAY,1,CAST(@p3 AS DATE)) \
             ), last_pkg AS (SELECT code_machine, package_type FROM pkg_scan WHERE rn=1) \
             SELECT k.code_machine, se.job_type, se.datex, se.date_ack, se.date_close, se.des_job, \
                    se.wait_min, se.tech_name, COALESCE(se.package_type,lp.package_type) AS package_type, \
                    FORMAT(se.datex,'HH:mm') AS t_start_fmt, FORMAT(se.date_close,'HH:mm') AS t_end_fmt \
             FROM key_mc k LEFT JOIN shift_ev se ON k.code_machine=se.code_machine \
             LEFT JOIN last_pkg lp ON k.code_machine=lp.code_machine ORDER BY k.code_machine, se.datex",
            self.machine_table, self.view, self.view
        );
        let sql_open = "SELECT RTRIM(LTRIM(code_machine)) AS code_machine, job_type, des_job, \
            FORMAT(datex,'HH:mm') AS t_start, DATEDIFF(MINUTE,datex,GETDATE()) AS dur_min \
            FROM dbo.job_list WHERE id_operation='WB' AND date_close IS NULL \
            AND code_machine!='' AND LEN(code_machine)>3".to_string();

        let (ev_rows, open_rows) = tokio::try_join!(
            exec(self.pool, &sql_ev, &params_ev),
            exec(self.pool, &sql_open, &params_open),
        )?;

        Ok(build_shift_report(&ev_rows, &open_rows, packages, shift, &time_range))
    }
}

/// Shared shift-report logic used by both WB and DA repos
pub fn build_shift_report(
    ev_rows: &[tiberius::Row],
    open_rows: &[tiberius::Row],
    packages: &str,
    shift: &str,
    time_range: &str,
) -> Value {
    let pkg_list: Vec<&str> = packages.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let has_all = pkg_list.is_empty() || pkg_list.contains(&"__ALL__");
    let has_qfn = pkg_list.contains(&"__QFN__");
    let pkg_set: HashSet<String> = pkg_list.iter().filter(|p| !p.starts_with("__")).map(|s| s.to_string()).collect();

    // Build machine → last package map
    let mut machine_pkg: HashMap<String, String> = Default::default();
    for r in ev_rows {
        let mid = str_val(r, "code_machine");
        if let Some(pkg) = opt_str(r, "package_type") {
            if !pkg.trim().is_empty() { machine_pkg.insert(mid, pkg.trim().to_string()); }
        }
    }

    let all_machines: Vec<String> = {
        let mut v: Vec<String> = ev_rows.iter().map(|r| str_val(r, "code_machine")).collect::<HashSet<_>>().into_iter().collect();
        v.sort();
        v
    };

    let (target, pkg_label) = if has_all {
        let mut v: Vec<String> = machine_pkg.keys().cloned().collect(); v.sort();
        (v, "All Packages".to_string())
    } else if has_qfn {
        let v: Vec<String> = all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| p.to_uppercase().contains("QFN")))
            .cloned().collect();
        (v, "All QFN".to_string())
    } else {
        let v: Vec<String> = all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| pkg_set.contains(p)))
            .cloned().collect();
        let label = if pkg_set.len() <= 2 {
            let mut s: Vec<&String> = pkg_set.iter().collect(); s.sort(); s.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        } else { format!("{} packages", pkg_set.len()) };
        (v, label)
    };

    // Group events by machine
    let mut closed_by: HashMap<String, Vec<&tiberius::Row>> = Default::default();
    for r in ev_rows {
        if r.get::<&str, _>("job_type").is_none() { continue; } // null row = no events
        closed_by.entry(str_val(r, "code_machine")).or_default().push(r);
    }
    let mut open_by: HashMap<String, Vec<&tiberius::Row>> = Default::default();
    for r in open_rows {
        open_by.entry(str_val(r, "code_machine")).or_default().push(r);
    }

    let mut machine_rows: Vec<Value> = Vec::new();
    let mut tech_set: HashSet<String> = Default::default();

    for mid in &target {
        let (mut wait_down, mut down_min, mut wait_setup, mut setup_min) = (0.0f64, 0.0, 0.0, 0.0);
        let (mut setup_conv_min, mut sbo_min) = (0.0f64, 0.0);
        let mut events: Vec<Value> = Vec::new();

        for r in closed_by.get(mid.as_str()).iter().flat_map(|v| v.iter()) {
            let jt = str_val(r, "job_type").to_uppercase();
            let wt = f64_val(r, "wait_min");
            let datex     = r.get::<NaiveDateTime, _>("datex");
            let date_ack  = r.get::<NaiveDateTime, _>("date_ack");
            let date_close= r.get::<NaiveDateTime, _>("date_close");
            let repair = if let (Some(ack), Some(cl)) = (date_ack, date_close) {
                if cl > ack { ((cl - ack).num_seconds() as f64 / 60.0).round() } else { 0.0 }
            } else if let (Some(dx), Some(cl)) = (datex, date_close) {
                ((cl - dx).num_seconds() as f64 / 60.0 - wt).max(0.0).round()
            } else { 0.0 };

            if jt == "M/C DOWN" { wait_down += wt; down_min += repair; }
            else if LOST_TYPES.contains(&jt.as_str()) {
                wait_setup += wt; setup_min += repair;
                if jt == "SETUP BY OPERATOR" { sbo_min += repair; }
                if SETUP_CONV.contains(&jt.as_str()) { setup_conv_min += repair; }
            }
            if let Some(tech) = opt_str(r, "tech_name") {
                if !tech.is_empty() && TECH_JT.contains(&jt.as_str()) { tech_set.insert(tech); }
            }
            events.push(json!({
                "job_type": str_val(r, "job_type"),
                "t_start":  str_val(r, "t_start_fmt"),
                "t_end":    str_val(r, "t_end_fmt"),
                "des_job":  str_val(r, "des_job"),
                "dur_min":  repair + wt,
                "is_open":  false,
            }));
        }

        for r in open_by.get(mid.as_str()).iter().flat_map(|v| v.iter()) {
            let jt = str_val(r, "job_type").to_uppercase();
            let dur = f64_val(r, "dur_min");
            if jt == "M/C DOWN" { wait_down += dur; }
            else if LOST_TYPES.contains(&jt.as_str()) { wait_setup += dur; }
            events.push(json!({
                "job_type": str_val(r, "job_type"),
                "t_start":  str_val(r, "t_start"),
                "t_end":    "",
                "des_job":  str_val(r, "des_job"),
                "dur_min":  dur,
                "is_open":  true,
            }));
        }

        let total_loss = wait_down + down_min + wait_setup + setup_min;
        let util_pct = ((SHIFT_MIN - total_loss) / SHIFT_MIN * 100.0).clamp(0.0, 100.0);
        let util_pct = (util_pct * 10.0).round() / 10.0;

        machine_rows.push(json!({
            "machine_id":     mid,
            "package":        machine_pkg.get(mid.as_str()).cloned().unwrap_or_default(),
            "util_pct":       util_pct,
            "wait_down_min":  wait_down,
            "down_min":       down_min,
            "wait_setup_min": wait_setup,
            "setup_min":      setup_min,
            "setup_conv_min": setup_conv_min,
            "sbo_min":        sbo_min,
            "total_loss_min": total_loss,
            "events":         events,
        }));
    }

    machine_rows.sort_by(|a, b| {
        a["util_pct"].as_f64().unwrap_or(0.0)
            .partial_cmp(&b["util_pct"].as_f64().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let n = machine_rows.len() as f64;
    let fleet_min = n * SHIFT_MIN;
    let avg_util = if n > 0.0 { machine_rows.iter().map(|r| r["util_pct"].as_f64().unwrap_or(0.0)).sum::<f64>() / n } else { 0.0 };
    let sum_f = |key: &str| machine_rows.iter().map(|r| r[key].as_f64().unwrap_or(0.0)).sum::<f64>();
    let pct   = |m: f64| if fleet_min > 0.0 { ((m / fleet_min * 100.0) * 10.0).round() / 10.0 } else { 0.0 };

    let kpi = json!({
        "total":          machine_rows.len(),
        "n_down":         machine_rows.iter().filter(|r| r["down_min"].as_f64().unwrap_or(0.0) > 0.0).count(),
        "n_setup":        machine_rows.iter().filter(|r| r["down_min"].as_f64().unwrap_or(0.0) == 0.0 && (r["setup_min"].as_f64().unwrap_or(0.0) + r["wait_setup_min"].as_f64().unwrap_or(0.0)) > 0.0).count(),
        "n_full":         machine_rows.iter().filter(|r| r["total_loss_min"].as_f64().unwrap_or(0.0) == 0.0).count(),
        "n_low":          machine_rows.iter().filter(|r| r["util_pct"].as_f64().unwrap_or(0.0) < 85.0).count(),
        "avg_util":       ((avg_util * 10.0).round() / 10.0),
        "down_pct":       pct(sum_f("down_min")),
        "wait_pct":       pct(sum_f("wait_down_min") + sum_f("wait_setup_min")),
        "setup_conv_pct": pct(sum_f("setup_conv_min")),
        "sbo_pct":        pct(sum_f("sbo_min")),
        "n_tech":         tech_set.len(),
    });

    json!({
        "machines":   machine_rows,
        "kpi":        kpi,
        "pkg_label":  pkg_label,
        "shift":      shift,
        "time_range": time_range,
    })
}
