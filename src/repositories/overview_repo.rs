use serde_json::{json, Value};
use std::collections::HashMap;
use chrono::Timelike;

use crate::{db::MssqlPool, errors::AppError, helpers::where_builder::parse_areas, oracle::OracleCache};
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

        let (kpi_sql, kpi_params, mat_sql, mat_params) = if area_list.is_empty() {
            let ks = format!(
                "SELECT \
                ((SELECT COUNT(*) FROM dbo.machine WHERE id_operation IS NOT NULL AND id_operation != '' \
                  AND id_operation != 'WB' AND flag_key = 1 AND ISNULL(flag_delete,0) != 1) \
                +(SELECT COUNT(*) FROM dbo.machine a WHERE a.id_operation = 'WB' AND a.flag_key = 1 \
                  AND ISNULL(a.flag_delete,0) != 1 AND a.code_machine NOT LIKE '%[LR]' \
                  AND NOT EXISTS (SELECT 1 FROM dbo.machine b WHERE b.id_operation = 'WB' \
                    AND (b.code_machine = a.code_machine + 'L' OR b.code_machine = a.code_machine + 'R'))) \
                +(SELECT COUNT(*) FROM dbo.machine a WHERE a.id_operation = 'WB' \
                  AND ISNULL(a.flag_delete,0) != 1 AND a.code_machine LIKE '%[LR]' \
                  AND EXISTS (SELECT 1 FROM dbo.machine b WHERE b.id_operation = 'WB' AND b.flag_key = 1 \
                    AND b.code_machine = LEFT(a.code_machine, LEN(a.code_machine)-1)))) AS total_key_machines, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NULL AND code_machine != '' AND date_ack IS NULL) AS waiting_count, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NULL AND code_machine != '' AND date_ack IS NOT NULL) AS on_process_count, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NULL AND code_machine != '' AND job_type = 'M/C DOWN') AS down_count, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NOT NULL AND code_machine != '' AND LEN(code_machine) > 3 \
                  AND date_close >= {}) AS closed_this_shift",
                shift_cutoff
            );
            let ms = "SELECT job_type, \
                SUM(CASE WHEN date_ack IS NULL AND date_close IS NULL THEN 1 ELSE 0 END) AS waiting, \
                SUM(CASE WHEN date_ack IS NOT NULL AND date_close IS NULL THEN 1 ELSE 0 END) AS on_process, \
                SUM(CASE WHEN date_close IS NOT NULL THEN 1 ELSE 0 END) AS closed, \
                COUNT(*) AS total \
                FROM dbo.job_list WHERE code_machine IS NOT NULL AND code_machine != '' AND LEN(code_machine) > 3 \
                GROUP BY job_type ORDER BY total DESC".to_string();
            (ks, vec![], ms, vec![])
        } else {
            let start = 1usize;
            let a_phs: Vec<String> = (start..start + area_list.len()).map(|i| format!("@p{}", i)).collect();
            let in_clause = a_phs.join(", ");
            let ks = format!(
                "SELECT \
                (SELECT COUNT(*) FROM {} WHERE id_operation IS NOT NULL AND id_operation != '' \
                  AND flag_key = 1 AND ISNULL(flag_delete,0) != 1 AND id_operation IN ({})) AS total_key_machines, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NULL AND code_machine != '' \
                  AND date_ack IS NULL AND id_operation IN ({})) AS waiting_count, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NULL AND code_machine != '' \
                  AND date_ack IS NOT NULL AND id_operation IN ({})) AS on_process_count, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NULL AND code_machine != '' \
                  AND job_type = 'M/C DOWN' AND id_operation IN ({})) AS down_count, \
                (SELECT COUNT(*) FROM dbo.job_list WHERE date_close IS NOT NULL AND code_machine != '' \
                  AND LEN(code_machine) > 3 AND id_operation IN ({}) AND date_close >= {}) AS closed_this_shift",
                mt, in_clause, in_clause, in_clause, in_clause, in_clause, shift_cutoff
            );
            let ms = format!(
                "SELECT job_type, \
                SUM(CASE WHEN date_ack IS NULL AND date_close IS NULL THEN 1 ELSE 0 END) AS waiting, \
                SUM(CASE WHEN date_ack IS NOT NULL AND date_close IS NULL THEN 1 ELSE 0 END) AS on_process, \
                SUM(CASE WHEN date_close IS NOT NULL THEN 1 ELSE 0 END) AS closed, \
                COUNT(*) AS total \
                FROM dbo.job_list WHERE code_machine IS NOT NULL AND code_machine != '' \
                AND LEN(code_machine) > 3 AND id_operation IN ({}) \
                GROUP BY job_type ORDER BY total DESC",
                in_clause
            );
            (ks, area_list.clone(), ms, area_list.clone())
        };

        let (kpi_rows, mat_rows) = tokio::try_join!(
            exec(self.pool, &kpi_sql, &kpi_params),
            exec(self.pool, &mat_sql, &mat_params),
        )?;

        // SQL kpi counts (total_machines from dbo.machine already includes ISO/FS;
        // job_list-derived counts exclude ISO/FS → supplemented from Oracle live below)
        let (total, mut down, mut wait, mut onproc, mut closed) = kpi_rows.first().map(|r| (
            i32_val(r, "total_key_machines"),
            i32_val(r, "down_count"),
            i32_val(r, "waiting_count"),
            i32_val(r, "on_process_count"),
            i32_val(r, "closed_this_shift"),
        )).unwrap_or((0, 0, 0, 0, 0));

        // status matrix map: job_type → [waiting, on_process, closed, total]
        let mut mat: HashMap<String, [i64; 4]> = HashMap::new();
        for r in &mat_rows {
            mat.insert(str_val(r, "job_type"), [
                i32_val(r, "waiting") as i64, i32_val(r, "on_process") as i64,
                i32_val(r, "closed") as i64, i32_val(r, "total") as i64,
            ]);
        }

        // ── Oracle live merge (ISO/FS) ──
        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            let ss = shift_start();
            for l in self.oracle.live_filtered(area_vec.as_deref()) {
                let open = l.status == "Waiting" || l.status == "On Process";
                if l.status == "Waiting" { wait += 1; }
                if l.status == "On Process" { onproc += 1; }
                if open && l.job_type == "M/C DOWN" { down += 1; }
                if l.status == "Closed" {
                    if let Some(dc) = l.date_close { if dc >= ss { closed += 1; } }
                }
                let e = mat.entry(l.job_type.clone()).or_insert([0, 0, 0, 0]);
                match l.status.as_str() {
                    "Waiting" => e[0] += 1,
                    "On Process" => e[1] += 1,
                    "Closed" => e[2] += 1,
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
}
