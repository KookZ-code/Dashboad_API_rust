use serde_json::{json, Value};
use std::collections::HashMap;
use chrono::Timelike;

use crate::{db::MssqlPool, errors::AppError, helpers::{kpi::r2, where_builder::parse_areas}, oracle::OracleCache};
use super::mssql_util::*;

pub struct OverviewRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub machine_table: String,
    pub oracle:        &'a OracleCache,
}

/// Start of the current shift (server-local), matching the SQL shift_cutoff logic.
fn shift_start() -> chrono::NaiveDateTime {
    let now = chrono::Local::now().naive_local();
    let h = now.time().hour();
    if (7..=18).contains(&h) {
        now.date().and_hms_opt(7, 0, 0).unwrap()
    } else if h >= 19 {
        now.date().and_hms_opt(19, 0, 0).unwrap()
    } else {
        (now.date() - chrono::Duration::days(1)).and_hms_opt(19, 0, 0).unwrap()
    }
}

impl<'a> OverviewRepo<'a> {
    pub fn new(pool: &'a MssqlPool, machine_table: impl Into<String>, oracle: &'a OracleCache) -> Self {
        Self { pool, machine_table: machine_table.into(), oracle }
    }

    pub async fn kpi_and_matrix(&self, areas: Option<&str>) -> Result<Value, AppError> {
        let mt = &self.machine_table;
        let area_list: Vec<String> = areas
            .map(crate::helpers::where_builder::parse_areas)
            .unwrap_or_default();

        let shift_cutoff = "CASE \
            WHEN DATEPART(HOUR, GETDATE()) BETWEEN 7 AND 18 \
            THEN CAST(CAST(GETDATE() AS DATE) AS DATETIME) + '07:00' \
            WHEN DATEPART(HOUR, GETDATE()) >= 19 \
            THEN CAST(CAST(GETDATE() AS DATE) AS DATETIME) + '19:00' \
            ELSE CAST(DATEADD(DAY,-1,CAST(GETDATE() AS DATE)) AS DATETIME) + '19:00' END";

        // total_key_machines comes from a different table (dbo.machine) so it stays
        // a separate scalar query; waiting/on_process/down/closed are now derived
        // ENTIRELY from the single matrix query below (summed across job types)
        // instead of running parallel COUNT(*) queries against dbo.job_list —
        // two queries hitting the same live table at slightly different instants
        // could disagree by however many jobs changed state in between (a real,
        // observed race, not just a rounding concern).
        let (ks, ms, params) = if area_list.is_empty() {
            let ks = "SELECT \
                ((SELECT COUNT(*) FROM dbo.machine WHERE id_operation IS NOT NULL AND id_operation != '' \
                  AND id_operation != 'WB' AND flag_key = 1 AND ISNULL(flag_delete,0) != 1) \
                +(SELECT COUNT(*) FROM dbo.machine a WHERE a.id_operation = 'WB' AND a.flag_key = 1 \
                  AND ISNULL(a.flag_delete,0) != 1 AND a.code_machine NOT LIKE '%[LR]' \
                  AND NOT EXISTS (SELECT 1 FROM dbo.machine b WHERE b.id_operation = 'WB' \
                    AND (b.code_machine = a.code_machine + 'L' OR b.code_machine = a.code_machine + 'R'))) \
                +(SELECT COUNT(*) FROM dbo.machine a WHERE a.id_operation = 'WB' \
                  AND ISNULL(a.flag_delete,0) != 1 AND a.code_machine LIKE '%[LR]' \
                  AND EXISTS (SELECT 1 FROM dbo.machine b WHERE b.id_operation = 'WB' AND b.flag_key = 1 \
                    AND b.code_machine = LEFT(a.code_machine, LEN(a.code_machine)-1)))) AS total_key_machines".to_string();
            let ms = format!("SELECT job_type, \
                SUM(CASE WHEN date_ack IS NULL AND date_close IS NULL AND job_type != 'SETUP BY OPERATOR' THEN 1 ELSE 0 END) AS waiting, \
                SUM(CASE WHEN date_ack IS NOT NULL AND date_close IS NULL AND job_type != 'SETUP BY OPERATOR' THEN 1 ELSE 0 END) AS on_process, \
                SUM(CASE WHEN date_close IS NOT NULL AND date_close >= {shift_cutoff} THEN 1 ELSE 0 END) AS closed, \
                COUNT(*) AS total \
                FROM dbo.job_list WHERE code_machine IS NOT NULL AND code_machine != '' AND LEN(code_machine) > 3 \
                GROUP BY job_type ORDER BY total DESC");
            (ks, ms, vec![])
        } else {
            let a_phs: Vec<String> = (1..=area_list.len()).map(|i| format!("@p{}", i)).collect();
            let in_clause = a_phs.join(", ");
            let ks = format!(
                "SELECT COUNT(*) AS total_key_machines FROM {} WHERE id_operation IS NOT NULL AND id_operation != '' \
                 AND flag_key = 1 AND ISNULL(flag_delete,0) != 1 AND id_operation IN ({})",
                mt, in_clause
            );
            let ms = format!(
                "SELECT job_type, \
                SUM(CASE WHEN date_ack IS NULL AND date_close IS NULL AND job_type != 'SETUP BY OPERATOR' THEN 1 ELSE 0 END) AS waiting, \
                SUM(CASE WHEN date_ack IS NOT NULL AND date_close IS NULL AND job_type != 'SETUP BY OPERATOR' THEN 1 ELSE 0 END) AS on_process, \
                SUM(CASE WHEN date_close IS NOT NULL AND date_close >= {shift_cutoff} THEN 1 ELSE 0 END) AS closed, \
                COUNT(*) AS total \
                FROM dbo.job_list WHERE code_machine IS NOT NULL AND code_machine != '' \
                AND LEN(code_machine) > 3 AND id_operation IN ({}) \
                GROUP BY job_type ORDER BY total DESC",
                in_clause
            );
            (ks, ms, area_list.clone())
        };

        let (kpi_rows, mat_rows) = tokio::try_join!(
            exec(self.pool, &ks, &params),
            exec(self.pool, &ms, &params),
        )?;

        let total = kpi_rows.first().map(|r| i32_val(r, "total_key_machines")).unwrap_or(0);

        // status matrix map: job_type → [waiting, on_process, closed, total]
        let mut mat: HashMap<String, [i64; 4]> = HashMap::new();
        for r in &mat_rows {
            mat.insert(str_val(r, "job_type"), [
                i32_val(r, "waiting") as i64, i32_val(r, "on_process") as i64,
                i32_val(r, "closed") as i64, i32_val(r, "total") as i64,
            ]);
        }

        // Fleet KPI counts derived from the matrix (single source of truth) —
        // down_count only counts OPEN M/C Down (waiting + on_process), matching
        // the pre-existing semantics of the old down_count scalar query.
        let mut down   = mat.get("M/C DOWN").map(|e| e[0] + e[1]).unwrap_or(0) as i32;
        let mut wait   = mat.values().map(|e| e[0]).sum::<i64>() as i32;
        let mut onproc = mat.values().map(|e| e[1]).sum::<i64>() as i32;
        let mut closed = mat.values().map(|e| e[2]).sum::<i64>() as i32;

        // ── Oracle live merge (ISO/FS) ──
        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            let ss = shift_start();
            for l in self.oracle.live_filtered(area_vec.as_deref()) {
                let open = l.status == "Waiting" || l.status == "On Process";
                // A "Closed" record only counts if it closed within this shift —
                // same cutoff the MSSQL matrix query applies. Previously this
                // guard existed only for the scalar `closed` count, not for the
                // matrix entry below, so the matrix total could exceed the KPI
                // total by however many pre-shift closed Oracle jobs existed.
                let counts_as_closed = l.status == "Closed"
                    && l.date_close.map(|dc| dc >= ss).unwrap_or(false);

                if l.status == "Waiting" { wait += 1; }
                if l.status == "On Process" { onproc += 1; }
                if open && l.job_type == "M/C DOWN" { down += 1; }
                if counts_as_closed { closed += 1; }

                let e = mat.entry(l.job_type.clone()).or_insert([0, 0, 0, 0]);
                match l.status.as_str() {
                    "Waiting" => e[0] += 1,
                    "On Process" => e[1] += 1,
                    "Closed" if counts_as_closed => e[2] += 1,
                    _ => {}
                }
                e[3] += 1;
            }
        }

        let kpi = json!({
            "total_machines":    total,
            "running":           0.max(total - down - wait - onproc),
            "down":              down,
            "waiting":           wait,
            "on_process":        onproc,
            "closed_this_shift": closed,
        });

        let mut matrix: Vec<Value> = mat.into_iter().map(|(job_type, c)| json!({
            "job_type":   job_type,
            "waiting":    c[0],
            "on_process": c[1],
            "closed":     c[2],
            "total":      c[3],
        })).collect();
        matrix.sort_by(|a, b| b["total"].as_i64().unwrap_or(0).cmp(&a["total"].as_i64().unwrap_or(0)));

        Ok(json!({
            "kpi":           kpi,
            "status_matrix": matrix,
            "updated_at":    chrono::Utc::now().to_rfc3339(),
        }))
    }

    pub async fn open_jobs(&self, areas: Option<&str>, job_type: Option<&str>) -> Result<Vec<Value>, AppError> {
        let area_list: Vec<String> = areas
            .map(crate::helpers::where_builder::parse_areas)
            .unwrap_or_default();
        let mut params: Vec<String> = Vec::new();
        let mut extra = String::new();

        if !area_list.is_empty() {
            let start = params.len() + 1;
            let phs: Vec<String> = (start..start + area_list.len()).map(|i| format!("@p{}", i)).collect();
            extra.push_str(&format!(" AND id_operation IN ({})", phs.join(", ")));
            params.extend(area_list);
        }
        if let Some(jt) = job_type {
            extra.push_str(&format!(" AND job_type = @p{}", params.len() + 1));
            params.push(jt.to_string());
        }

        let sql = format!(
            "SELECT code_machine, id_operation AS area, job_type, des_job, datex, date_ack, by_ack AS tech, \
             CASE WHEN date_ack IS NULL THEN DATEDIFF(MINUTE, datex, GETDATE()) \
                  ELSE DATEDIFF(MINUTE, datex, date_ack) END AS wait_min, \
             CASE WHEN date_ack IS NOT NULL THEN DATEDIFF(MINUTE, date_ack, GETDATE()) ELSE NULL END AS repair_min, \
             CASE WHEN date_ack IS NULL THEN 'Waiting' ELSE 'On Process' END AS status, \
             [mpc] AS die_mask, [Package Type] AS package_type, [Wire Type] AS wire_type \
             FROM dbo.job_list \
             WHERE date_close IS NULL AND code_machine IS NOT NULL AND code_machine != '' \
               AND LEN(code_machine) > 3 AND datex >= DATEADD(MONTH,-1,GETDATE()) {} \
             ORDER BY datex ASC",
            extra
        );

        let rows = exec(self.pool, &sql, &params).await?;
        let mut out: Vec<Value> = rows.iter().map(|r| json!({
            "code_machine": str_val(r, "code_machine"),
            "area":         str_val(r, "area"),
            "job_type":     str_val(r, "job_type"),
            "des_job":      opt_str(r, "des_job"),
            "datex":        opt_dt_str(r, "datex"),
            "date_ack":     opt_dt_str(r, "date_ack"),
            "tech":         opt_str(r, "tech"),
            "wait_min":     i64_val(r, "wait_min"),
            "repair_min":   i64_val(r, "repair_min"),
            "status":       str_val(r, "status"),
            "die_mask":     opt_str(r, "die_mask"),
            "package_type": opt_str(r, "package_type"),
            "wire_type":    opt_str(r, "wire_type"),
        })).collect();

        // ── Oracle live open jobs (ISO/FS) ──
        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            for l in self.oracle.live_filtered(area_vec.as_deref()) {
                if l.status != "Waiting" && l.status != "On Process" { continue; }
                if let Some(jt) = job_type { if l.job_type != jt { continue; } }
                out.push(json!({
                    "code_machine": l.machine_id,
                    "area":         l.area,
                    "job_type":     l.job_type,
                    "des_job":      l.des_job,
                    "datex":        Value::Null,
                    "date_ack":     Value::Null,
                    "tech":         l.badge,
                    "wait_min":     l.wait_min,
                    "repair_min":   l.repair_min,
                    "status":       l.status,
                    "die_mask":     l.die_mask,
                    "package_type": l.package_type,
                    "wire_type":    "",
                }));
            }
        }
        Ok(out)
    }

    /// MTBF (hours) per machine + fleet aggregate, derived from existing M/C DOWN events —
    /// no dedicated MTBF table. Fleet MTBF = total uptime / total failures (not an average
    /// of per-machine MTBFs, which would over-weight low-event machines).
    pub async fn mtbf(
        &self, view: &str,
        start: Option<&str>, end: Option<&str>, areas: Option<&str>, shift: Option<&str>,
    ) -> Result<Value, AppError> {
        use crate::helpers::where_builder::{build_where, WhereOpts};

        let wc = build_where(WhereOpts { start, end, areas, shift, machine_id: None, drill_day: None });
        let sql = format!(
            "SELECT [code_machine] AS machine_id, \
             SUM(DATEDIFF(MINUTE,[date_ack],[date_close])) AS down_min, \
             COUNT(*) AS down_events \
             FROM {} {} AND [job_type] = 'M/C DOWN' \
             GROUP BY [code_machine]",
            view, wc.sql
        );
        let rows = exec(self.pool, &sql, &wc.params).await?;

        // Span of the requested window in hours — defaults to the last 24h when no
        // start/end given, matching how other endpoints treat a missing date range.
        let span_hours: f64 = match (start, end) {
            (Some(s), Some(e)) => {
                let sd = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d");
                let ed = chrono::NaiveDate::parse_from_str(e, "%Y-%m-%d");
                match (sd, ed) {
                    (Ok(sd), Ok(ed)) => ((ed - sd).num_days() as f64 + 1.0) * 24.0,
                    _ => 24.0,
                }
            }
            _ => 24.0,
        };

        let mut machines: Vec<Value> = Vec::new();
        let mut total_down_min = 0.0f64;
        let mut total_events   = 0i64;

        for r in &rows {
            let down_min = f64_val(r, "down_min");
            let events   = i64_val(r, "down_events");
            total_down_min += down_min;
            total_events   += events;
            // A machine with 0 M/C DOWN events never appears in this GROUP BY, so
            // events > 0 here always — no divide-by-zero guard needed per row.
            let mtbf_hours = ((span_hours - down_min / 60.0) / events as f64).max(0.0);
            machines.push(json!({
                "machine_id":  str_val(r, "machine_id"),
                "mtbf_hours":  r2(mtbf_hours),
                "down_events": events,
            }));
        }
        machines.sort_by(|a, b|
            b["mtbf_hours"].as_f64().unwrap_or(0.0)
                .partial_cmp(&a["mtbf_hours"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal));

        // Fleet MTBF: undefined (no failures observed) → 0.0 sentinel, distinguished
        // from a real low MTBF by `fleet_down_events == 0` in the same payload.
        let fleet_mtbf_hours = if total_events > 0 {
            r2(((span_hours * rows.len() as f64) - total_down_min / 60.0) / total_events as f64)
        } else {
            0.0
        };

        Ok(json!({
            "fleet_mtbf_hours": fleet_mtbf_hours,
            "fleet_down_events": total_events,
            "machines": machines,
        }))
    }
}
