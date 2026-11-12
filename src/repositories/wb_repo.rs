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
        if pkgs.iter().any(|p| !p.to_uppercase().contains("FN")) {
            opts.push(json!({ "value": "__LEAD__", "label": "— All Lead —" }));
        }
        if pkgs.iter().any(|p| p.to_uppercase().contains("FN")) {
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
        let params_open = vec![shift_start.clone(), shift_end.clone()];

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
                 AND UPPER(ISNULL([cause],'')) NOT IN ('CANCEL', 'N/A') \
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
            FORMAT(datex,'HH:mm') AS t_start, \
            CASE WHEN date_close IS NOT NULL THEN DATEDIFF(MINUTE,datex,date_close) \
                 ELSE DATEDIFF(MINUTE,datex,GETDATE()) END AS dur_min, \
            ISNULL(by_perform, by_ack) AS tech_name \
            FROM dbo.job_listx WHERE id_operation='WB' \
            AND code_machine!='' AND LEN(code_machine)>3 \
            AND datex>=@p1 AND datex<@p2 \
            AND (date_close IS NULL OR UPPER(ISNULL([cause],'')) IN ('CANCEL', 'N/A'))".to_string();


            let (ev_rows, open_rows) = tokio::try_join!(
            exec(self.pool, &sql_ev, &params_ev),
            exec(self.pool, &sql_open, &params_open),
        )?;

        Ok(build_shift_report(&ev_rows, &open_rows, packages, shift, &time_range))
    }

    pub async fn report_7day(&self, end_date: &str, packages: &str) -> Result<Value, AppError> {
        self.report_7day_for("WB", end_date, packages).await
    }

    /// 7-day, 14-shift trend for one operation area ('WB' or 'DA'). Shared so DA and WB stay identical.
    pub async fn report_7day_for(&self, op: &str, end_date: &str, packages: &str) -> Result<Value, AppError> {
        // Generate 7 days backwards (14 shifts: Night, Day)
        let end = chrono::NaiveDate::parse_from_str(end_date, "%Y-%m-%d")
            .map_err(|_| AppError::BadRequest("Invalid date format".into()))?;

        let mut shifts_data = Vec::new();

        // Loop backwards 7 days: each day has Night (prev evening) then Day
        for day_offset in (0..7).rev() {
            let date = end - chrono::Duration::days(day_offset as i64);
            let date_str = date.format("%Y-%m-%d").to_string();

            // Night shift (19:00 prev day → 07:00 this day)
            let (night_start, night_end, _) = Self::shift_window(&date_str, "Night");

            // Day shift (07:00 → 19:00 same day)
            let (day_start, day_end, _) = Self::shift_window(&date_str, "Day");

            // Query Night shift
            let night_report = self.query_shift_metrics(op, &night_start, &night_end, &date_str, packages).await?;
            shifts_data.push((date_str.clone(), "Night", night_report));

            // Query Day shift
            let day_report = self.query_shift_metrics(op, &day_start, &day_end, &date_str, packages).await?;
            shifts_data.push((date_str, "Day", day_report));
        }

        // Aggregate into arrays
        let mut dates = Vec::new();
        let mut shifts = Vec::new();
        let mut shift_details = Vec::new();
        let mut avg_util = Vec::new();
        let mut down_pct = Vec::new();
        let mut wait_pct = Vec::new();
        let mut setup_conv_pct = Vec::new();
        let mut idle_pct = Vec::new();

        for (date, shift, metrics) in shifts_data {
            dates.push(json!(date));
            shifts.push(json!(shift));
            avg_util.push(metrics["avg_util"].clone());
            down_pct.push(metrics["down_pct"].clone());
            wait_pct.push(metrics["wait_pct"].clone());
            setup_conv_pct.push(metrics["setup_conv_pct"].clone());
            idle_pct.push(metrics["idle_pct"].clone());

            shift_details.push(json!({
                "date": date,
                "shift": shift,
                "total_request_jobs": metrics.get("total_request_jobs").cloned().unwrap_or(json!(0)),
                "request_down_jobs": metrics.get("request_down_jobs").cloned().unwrap_or(json!(0)),
                "request_setup_jobs": metrics.get("request_setup_jobs").cloned().unwrap_or(json!(0)),
                "total_closed_jobs": metrics.get("total_closed_jobs").cloned().unwrap_or(json!(0)),
                "tech_count": metrics.get("tech_count").cloned().unwrap_or(json!(0)),
                "avg_job_per_tech": metrics.get("avg_job_per_tech").cloned().unwrap_or(json!(0.0)),
                "down_jobs": metrics.get("down_jobs").cloned().unwrap_or(json!(0)),
                "avg_down_time": metrics.get("avg_down_time").cloned().unwrap_or(json!(0.0)),
                "setup_jobs": metrics.get("setup_jobs").cloned().unwrap_or(json!(0)),
                "avg_setup_time": metrics.get("avg_setup_time").cloned().unwrap_or(json!(0.0)),
                "avg_waiting_time": metrics.get("avg_waiting_time").cloned().unwrap_or(json!(0.0)),
                "max_waiting_time": metrics.get("max_waiting_time").cloned().unwrap_or(json!(0.0))
            }));
        }

        Ok(json!({
            "dates": dates,
            "shifts": shifts,
            "shift_details": shift_details,
            "metrics": {
                "avg_util": avg_util,
                "down_pct": down_pct,
                "wait_pct": wait_pct,
                "setup_conv_pct": setup_conv_pct,
                "idle_pct": idle_pct
            }
        }))
    }

    pub async fn query_shift_metrics(&self, op: &str, start: &str, end: &str, date: &str, packages: &str) -> Result<Value, AppError> {
        let params_ev = vec![start.to_string(), end.to_string(), date.to_string()];
        let params_open = vec![start.to_string(), end.to_string()];

        let sql_ev = format!(
            "WITH key_mc AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine FROM {} \
               WHERE [id_operation] = '{op}' AND [flag_key] = 1 AND ISNULL([flag_delete],0) != 1 \
             ), shift_ev AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine, [job_type], [datex], [date_ack], \
                      [date_close], [des_job], ISNULL([Waiting_time],0) AS wait_min, [Package Type] AS package_type, \
                      NULLIF(RTRIM(LTRIM(ISNULL([by_perform],[by_ack]))),'') AS tech_name \
               FROM {} WHERE [id_operation]='{op}' AND [datex]>=@p1 AND [datex]<@p2 \
                 AND [date_close] IS NOT NULL AND [date_close]>[datex] \
                 AND UPPER(ISNULL([cause],'')) NOT IN ('CANCEL', 'N/A') \
             ), pkg_scan AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine, [Package Type] AS package_type, \
                      ROW_NUMBER() OVER (PARTITION BY RTRIM(LTRIM([code_machine])) ORDER BY [datex] DESC) AS rn \
               FROM {} WHERE [id_operation]='{op}' AND [Package Type] IS NOT NULL AND [Package Type]!='' \
                 AND [datex]>=DATEADD(DAY,-7,CAST(@p3 AS DATE)) AND [datex]<DATEADD(DAY,1,CAST(@p3 AS DATE)) \
             ), last_pkg AS (SELECT code_machine, package_type FROM pkg_scan WHERE rn=1) \
             SELECT k.code_machine, se.job_type, se.datex, se.date_ack, se.date_close, se.des_job, \
                    se.wait_min, se.tech_name, COALESCE(se.package_type,lp.package_type) AS package_type \
             FROM key_mc k LEFT JOIN shift_ev se ON k.code_machine=se.code_machine \
             LEFT JOIN last_pkg lp ON k.code_machine=lp.code_machine ORDER BY k.code_machine, se.datex",
            self.machine_table, self.view, self.view
        ).replace("{op}", op);
        let sql_open = "SELECT RTRIM(LTRIM(code_machine)) AS code_machine, job_type, des_job, \
            FORMAT(datex,'HH:mm') AS t_start, \
            CASE WHEN date_close IS NOT NULL THEN DATEDIFF(MINUTE,datex,date_close) \
                 ELSE DATEDIFF(MINUTE,datex,GETDATE()) END AS dur_min, \
            ISNULL(by_perform, by_ack) AS tech_name \
            FROM dbo.job_listx WHERE id_operation='{op}' \
            AND code_machine!='' AND LEN(code_machine)>3 \
            AND datex>=@p1 AND datex<@p2 \
            AND (date_close IS NULL OR UPPER(ISNULL([cause],'')) IN ('CANCEL', 'N/A'))".replace("{op}", op);

        let (ev_rows, open_rows) = tokio::try_join!(
            exec(self.pool, &sql_ev, &params_ev),
            exec(self.pool, &sql_open, &params_open),
        )?;

        Ok(compute_shift_metrics_with_details(&ev_rows, &open_rows, packages))
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
    let has_lead = pkg_list.contains(&"__LEAD__");
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
    } else if has_lead {
        let v: Vec<String> = all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| !p.to_uppercase().contains("FN")))
            .cloned().collect();
        (v, "All Lead".to_string())
    } else if has_qfn {
        let v: Vec<String> = all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| p.to_uppercase().contains("FN")))
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

    #[derive(Default)]
    struct TechStats {
        closed_job_ids: HashSet<String>,
        open_job_ids: HashSet<String>,
        down_job_ids: HashSet<String>,
        setup_job_ids: HashSet<String>,
        down_times: Vec<f64>,
        setup_times: Vec<f64>,
        down_events: i32,
        setup_events: i32,
    }
    let mut tech_stats: HashMap<String, TechStats> = Default::default();

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

            if jt == "M/C DOWN" {
                wait_down += wt;
                down_min += repair;
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() {
                        // Type-prefixed so a DOWN and a SETUP starting at the same datex count as two jobs
                        let job_id = format!("DOWN:{:?}", datex);
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.down_times.push(repair);
                        ts.down_events += 1;
                        ts.down_job_ids.insert(job_id.clone());
                        ts.closed_job_ids.insert(job_id);
                        tech_set.insert(tech);
                    }
                }
            } else if LOST_TYPES.contains(&jt.as_str()) {
                wait_setup += wt; setup_min += repair;
                if jt == "SETUP BY OPERATOR" { sbo_min += repair; }
                if SETUP_CONV.contains(&jt.as_str()) { setup_conv_min += repair; }
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() && SETUP_CONV.contains(&jt.as_str()) {
                        let job_id = format!("SETUP:{:?}", datex);
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.setup_times.push(repair);
                        ts.setup_events += 1;
                        ts.setup_job_ids.insert(job_id.clone());
                        ts.closed_job_ids.insert(job_id);
                        tech_set.insert(tech);
                    }
                }
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
            if jt == "M/C DOWN" {
                wait_down += dur;
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() {
                        let job_id = format!("{}:{}:{}", mid, str_val(r, "t_start"), str_val(r, "des_job"));
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.open_job_ids.insert(job_id);
                        tech_set.insert(tech);
                    }
                }
            } else if LOST_TYPES.contains(&jt.as_str()) {
                wait_setup += dur;
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() && SETUP_CONV.contains(&jt.as_str()) {
                        let job_id = format!("{}:{}:{}", mid, str_val(r, "t_start"), str_val(r, "des_job"));
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.open_job_ids.insert(job_id);
                        tech_set.insert(tech);
                    }
                }
            }
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

    // Compute technician stats
    let mut tech_list: Vec<Value> = Vec::new();
    let mut total_closed: i64 = 0;
    let mut total_open: i64 = 0;
    let mut total_down_jobs: i64 = 0;
    let mut total_setup_jobs: i64 = 0;
    let mut down_avgs: Vec<f64> = Vec::new();
    let mut setup_avgs: Vec<f64> = Vec::new();
    let mut down_all_mins: Vec<f64> = Vec::new();
    let mut down_all_maxs: Vec<f64> = Vec::new();
    let mut setup_all_mins: Vec<f64> = Vec::new();
    let mut setup_all_maxs: Vec<f64> = Vec::new();

    for (tech_name, stats) in tech_stats.iter() {
        let down_avg = if stats.down_times.is_empty() { 0.0 } else { stats.down_times.iter().sum::<f64>() / stats.down_times.len() as f64 };
        let setup_avg = if stats.setup_times.is_empty() { 0.0 } else { stats.setup_times.iter().sum::<f64>() / stats.setup_times.len() as f64 };
        let down_min = stats.down_times.iter().cloned().fold(f64::INFINITY, f64::min);
        let down_max = stats.down_times.iter().cloned().fold(0.0, f64::max);
        let setup_min = stats.setup_times.iter().cloned().fold(f64::INFINITY, f64::min);
        let setup_max = stats.setup_times.iter().cloned().fold(0.0, f64::max);

        let closed_count = stats.closed_job_ids.len() as i64;
        let open_count = stats.open_job_ids.len() as i64;
        let down_count = stats.down_job_ids.len() as i64;
        let setup_count = stats.setup_job_ids.len() as i64;
        let total_jobs = closed_count + open_count;

        total_closed += closed_count;
        total_open += open_count;
        total_down_jobs += down_count;
        total_setup_jobs += setup_count;
        down_avgs.push(down_avg);
        setup_avgs.push(setup_avg);
        if down_min != f64::INFINITY { down_all_mins.push(down_min); }
        down_all_maxs.push(down_max);
        if setup_min != f64::INFINITY { setup_all_mins.push(setup_min); }
        setup_all_maxs.push(setup_max);

        tech_list.push(json!({
            "name": tech_name,
            "closed_jobs": closed_count,
            "open_jobs": open_count,
            "total_jobs": total_jobs,
            "down_count": down_count,
            "setup_count": setup_count,
            "down_avg": ((down_avg * 10.0).round() / 10.0),
            "down_min": if down_min == f64::INFINITY { 0.0 } else { ((down_min * 10.0).round() / 10.0) },
            "down_max": ((down_max * 10.0).round() / 10.0),
            "down_events": stats.down_events,
            "setup_avg": ((setup_avg * 10.0).round() / 10.0),
            "setup_min": if setup_min == f64::INFINITY { 0.0 } else { ((setup_min * 10.0).round() / 10.0) },
            "setup_max": ((setup_max * 10.0).round() / 10.0),
            "setup_events": stats.setup_events,
        }));
    }

    let tech_summary = if !tech_list.is_empty() {
        let total_all: i64 = total_closed + total_open;
        let closed_pct = if total_all > 0 { (total_closed as f64 / total_all as f64 * 100.0 * 10.0).round() / 10.0 } else { 0.0 };
        let open_pct = if total_all > 0 { (total_open as f64 / total_all as f64 * 100.0 * 10.0).round() / 10.0 } else { 0.0 };
        let avg_jobs = (total_all as f64 / tech_list.len() as f64 * 10.0).round() / 10.0;
        let tech_count = tech_list.len() as i64;

        json!({
            "tech_count": tech_count,
            "down_jobs": total_down_jobs,
            "setup_jobs": total_setup_jobs,
            "closed_jobs": total_closed,
            "open_jobs": total_open,
            "total_jobs": total_all,
            "closed_pct": closed_pct,
            "open_pct": open_pct,
            "avg_jobs": avg_jobs,
            "down_avg": ((down_avgs.iter().sum::<f64>() / down_avgs.len() as f64 * 10.0).round() / 10.0),
            "down_min": if down_all_mins.is_empty() { 0.0 } else { ((down_all_mins.iter().cloned().fold(f64::INFINITY, f64::min) * 10.0).round() / 10.0) },
            "down_max": ((down_all_maxs.iter().cloned().fold(0.0, f64::max) * 10.0).round() / 10.0),
            "setup_avg": ((setup_avgs.iter().sum::<f64>() / setup_avgs.len() as f64 * 10.0).round() / 10.0),
            "setup_min": if setup_all_mins.is_empty() { 0.0 } else { (setup_all_mins.iter().cloned().fold(f64::INFINITY, f64::min) * 10.0).round() / 10.0 },
            "setup_max": ((setup_all_maxs.iter().cloned().fold(0.0, f64::max) * 10.0).round() / 10.0),
        })
    } else {
        json!({
            "tech_count": 0,
            "down_jobs": 0,
            "setup_jobs": 0,
            "closed_jobs": 0,
            "open_jobs": 0,
            "total_jobs": 0,
            "closed_pct": 0.0,
            "open_pct": 0.0,
            "avg_jobs": 0.0,
            "down_avg": 0.0,
            "down_min": 0.0,
            "down_max": 0.0,
            "setup_avg": 0.0,
            "setup_min": 0.0,
            "setup_max": 0.0,
        })
    };

    json!({
        "machines":      machine_rows,
        "kpi":           kpi,
        "pkg_label":     pkg_label,
        "shift":         shift,
        "time_range":    time_range,
        "tech_summary":  tech_summary,
        "tech_list":     tech_list,
    })
}

fn compute_shift_metrics_with_details(
    ev_rows: &[tiberius::Row],
    open_rows: &[tiberius::Row],
    packages: &str,
) -> Value {
    use super::mssql_util::*;

    const LOST_TYPES: &[&str] = &[
        "SETUP", "SETUP BY OPERATOR", "CONVERT", "CLEAN MOLD",
        "CHANGE CAP", "FACILITY DOWN", "ENGINEERING DOWN",
    ];
    const SETUP_CONV: &[&str] = &["SETUP", "CONVERT", "CLEAN MOLD", "CHANGE CAP"];
    const SHIFT_MIN: f64 = 720.0;

    let pkg_list: Vec<&str> = packages.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let has_all = pkg_list.is_empty() || pkg_list.contains(&"__ALL__");
    let has_lead = pkg_list.contains(&"__LEAD__");
    let has_qfn = pkg_list.contains(&"__QFN__");
    let pkg_set: HashSet<String> = pkg_list.iter().filter(|p| !p.starts_with("__")).map(|s| s.to_string()).collect();

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

    let target: Vec<String> = if has_all {
        let mut v: Vec<String> = machine_pkg.keys().cloned().collect(); v.sort(); v
    } else if has_lead {
        all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| !p.to_uppercase().contains("FN")))
            .cloned().collect()
    } else if has_qfn {
        all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| p.to_uppercase().contains("FN")))
            .cloned().collect()
    } else {
        all_machines.iter()
            .filter(|m| machine_pkg.get(*m).is_some_and(|p| pkg_set.contains(p)))
            .cloned().collect()
    };

    let mut closed_by: HashMap<String, Vec<&tiberius::Row>> = Default::default();
    for r in ev_rows {
        if r.get::<&str, _>("job_type").is_none() { continue; }
        closed_by.entry(str_val(r, "code_machine")).or_default().push(r);
    }
    let mut open_by: HashMap<String, Vec<&tiberius::Row>> = Default::default();
    for r in open_rows {
        open_by.entry(str_val(r, "code_machine")).or_default().push(r);
    }

    #[derive(Default)]
    struct TechStats {
        closed_job_ids: HashSet<String>,
        open_job_ids: HashSet<String>,
        down_job_ids: HashSet<String>,
        setup_job_ids: HashSet<String>,
        down_times: Vec<f64>,
        setup_times: Vec<f64>,
        wait_times: Vec<f64>,
        // Open (not yet closed) jobs split by type, so the 7-day chart can show request per type
        open_down_job_ids: HashSet<String>,
        open_setup_job_ids: HashSet<String>,
    }
    let mut tech_stats: HashMap<String, TechStats> = Default::default();
    let mut wait_times_all: Vec<f64> = Vec::new();

    let mut total_util_pct = 0.0f64;
    let mut total_down_pct = 0.0f64;
    let mut total_wait_pct = 0.0f64;
    let mut total_setup_conv_pct = 0.0f64;
    let mut total_idle_pct = 0.0f64;

    for mid in &target {
        let (mut wait_down, mut down_min, mut wait_setup, mut setup_min) = (0.0f64, 0.0, 0.0, 0.0);
        let (mut setup_conv_min, mut sbo_min) = (0.0f64, 0.0);

        for r in closed_by.get(mid.as_str()).iter().flat_map(|v| v.iter()) {
            let jt = str_val(r, "job_type").to_uppercase();
            let wt = f64_val(r, "wait_min");
            let datex     = r.get::<chrono::NaiveDateTime, _>("datex");
            let date_ack  = r.get::<chrono::NaiveDateTime, _>("date_ack");
            let date_close= r.get::<chrono::NaiveDateTime, _>("date_close");
            let repair = if let (Some(ack), Some(cl)) = (date_ack, date_close) {
                if cl > ack { ((cl - ack).num_seconds() as f64 / 60.0).round() } else { 0.0 }
            } else if let (Some(dx), Some(cl)) = (datex, date_close) {
                ((cl - dx).num_seconds() as f64 / 60.0 - wt).max(0.0).round()
            } else { 0.0 };

            wait_times_all.push(wt);

            if jt == "M/C DOWN" {
                wait_down += wt;
                down_min += repair;
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() && tech.starts_with('B') {
                        // Type-prefixed so a DOWN and a SETUP starting at the same datex count as two jobs
                        let job_id = format!("DOWN:{:?}", datex);
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.down_times.push(repair);
                        ts.wait_times.push(wt);
                        ts.down_job_ids.insert(job_id.clone());
                        ts.closed_job_ids.insert(job_id);
                    }
                }
            } else if LOST_TYPES.contains(&jt.as_str()) {
                wait_setup += wt;
                setup_min += repair;
                if jt == "SETUP BY OPERATOR" { sbo_min += repair; }
                if SETUP_CONV.contains(&jt.as_str()) { setup_conv_min += repair; }
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() && SETUP_CONV.contains(&jt.as_str()) && tech.starts_with('B') {
                        let job_id = format!("SETUP:{:?}", datex);
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.setup_times.push(repair);
                        ts.wait_times.push(wt);
                        ts.setup_job_ids.insert(job_id.clone());
                        ts.closed_job_ids.insert(job_id);
                    }
                }
            }
        }

        for r in open_by.get(mid.as_str()).iter().flat_map(|v| v.iter()) {
            let jt = str_val(r, "job_type").to_uppercase();
            let dur = f64_val(r, "dur_min");
            if jt == "M/C DOWN" {
                wait_down += dur;
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() && tech.starts_with('B') {
                        let job_id = format!("{}:{}:{}", mid, str_val(r, "t_start"), str_val(r, "des_job"));
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.open_job_ids.insert(job_id.clone());
                        ts.open_down_job_ids.insert(job_id);
                    }
                }
            } else if LOST_TYPES.contains(&jt.as_str()) {
                wait_setup += dur;
                if let Some(tech) = opt_str(r, "tech_name") {
                    if !tech.is_empty() && SETUP_CONV.contains(&jt.as_str()) && tech.starts_with('B') {
                        let job_id = format!("{}:{}:{}", mid, str_val(r, "t_start"), str_val(r, "des_job"));
                        let ts = tech_stats.entry(tech.clone()).or_default();
                        ts.open_job_ids.insert(job_id.clone());
                        ts.open_setup_job_ids.insert(job_id);
                    }
                }
            }
        }

        let total_loss = wait_down + down_min + wait_setup + setup_min;
        let util_pct = ((SHIFT_MIN - total_loss) / SHIFT_MIN * 100.0).clamp(0.0, 100.0);
        let util_pct = (util_pct * 10.0).round() / 10.0;

        total_util_pct += util_pct;
        // down_pct = repair time only (matches the fleet-level calc below in
        // this file, used by the per-shift table); wait_down previously got
        // added into BOTH down_pct and wait_pct, double-counting it and
        // inflating the M/C DOWN % line in the 7-day trend chart above the
        // fleet KPI card's % for the same shift.
        total_down_pct += down_min / SHIFT_MIN * 100.0;
        total_wait_pct += (wait_down + wait_setup) / SHIFT_MIN * 100.0;
        total_setup_conv_pct += setup_conv_min / SHIFT_MIN * 100.0;
        total_idle_pct += sbo_min / SHIFT_MIN * 100.0;
    }

    let total_closed_jobs: i32 = tech_stats.values().map(|ts| ts.closed_job_ids.len() as i32).sum();
    let total_open_jobs: i32 = tech_stats.values().map(|ts| ts.open_job_ids.len() as i32).sum();
    let total_request_jobs: i32 = total_closed_jobs + total_open_jobs;
    let total_down_jobs: i32 = tech_stats.values().map(|ts| ts.down_job_ids.len() as i32).sum();
    let total_setup_jobs: i32 = tech_stats.values().map(|ts| ts.setup_job_ids.len() as i32).sum();
    // Request per type = closed of that type + open of that type (closed keys and open keys never overlap)
    let request_down_jobs: i32 = tech_stats.values().map(|ts| (ts.down_job_ids.len() + ts.open_down_job_ids.len()) as i32).sum();
    let request_setup_jobs: i32 = tech_stats.values().map(|ts| (ts.setup_job_ids.len() + ts.open_setup_job_ids.len()) as i32).sum();
    let tech_count = tech_stats.len() as i32;

    let avg_down_time = if !tech_stats.is_empty() {
        let all_down_times: Vec<f64> = tech_stats.values().flat_map(|ts| ts.down_times.iter().copied()).collect();
        if !all_down_times.is_empty() {
            (all_down_times.iter().sum::<f64>() / all_down_times.len() as f64 * 10.0).round() / 10.0
        } else { 0.0 }
    } else { 0.0 };

    let avg_setup_time = if !tech_stats.is_empty() {
        let all_setup_times: Vec<f64> = tech_stats.values().flat_map(|ts| ts.setup_times.iter().copied()).collect();
        if !all_setup_times.is_empty() {
            (all_setup_times.iter().sum::<f64>() / all_setup_times.len() as f64 * 10.0).round() / 10.0
        } else { 0.0 }
    } else { 0.0 };

    let (avg_wait_time, max_wait_time) = if !wait_times_all.is_empty() {
        let jobs_with_wait: Vec<f64> = wait_times_all.iter().filter(|&&w| w > 0.0).copied().collect();
        let avg = if !jobs_with_wait.is_empty() {
            (jobs_with_wait.iter().sum::<f64>() / jobs_with_wait.len() as f64 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let max = wait_times_all.iter().max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)).copied().unwrap_or(0.0);
        (avg, max)
    } else { (0.0, 0.0) };

    let n = target.len() as f64;
    if n > 0.0 {
        json!({
            "avg_util": ((total_util_pct / n) * 10.0).round() / 10.0,
            "down_pct": ((total_down_pct / n) * 10.0).round() / 10.0,
            "wait_pct": ((total_wait_pct / n) * 10.0).round() / 10.0,
            "setup_conv_pct": ((total_setup_conv_pct / n) * 10.0).round() / 10.0,
            "idle_pct": ((total_idle_pct / n) * 10.0).round() / 10.0,
            "total_request_jobs": total_request_jobs,
            "request_down_jobs": request_down_jobs,
            "request_setup_jobs": request_setup_jobs,
            "total_closed_jobs": total_closed_jobs,
            "tech_count": tech_count,
            "avg_job_per_tech": if tech_count > 0 { ((total_closed_jobs as f64 / tech_count as f64) * 10.0).round() / 10.0 } else { 0.0 },
            "down_jobs": total_down_jobs,
            "avg_down_time": avg_down_time,
            "setup_jobs": total_setup_jobs,
            "avg_setup_time": avg_setup_time,
            "avg_waiting_time": avg_wait_time,
            "max_waiting_time": max_wait_time
        })
    } else {
        json!({
            "avg_util": 0.0,
            "down_pct": 0.0,
            "wait_pct": 0.0,
            "setup_conv_pct": 0.0,
            "idle_pct": 0.0,
            "total_request_jobs": 0,
            "request_down_jobs": 0,
            "request_setup_jobs": 0,
            "total_closed_jobs": 0,
            "tech_count": 0,
            "avg_job_per_tech": 0.0,
            "down_jobs": 0,
            "avg_down_time": 0.0,
            "setup_jobs": 0,
            "avg_setup_time": 0.0,
            "avg_waiting_time": 0.0,
            "max_waiting_time": 0.0
        })
    }
}

