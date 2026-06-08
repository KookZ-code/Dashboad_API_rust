use serde_json::{json, Value};

use crate::{db::MssqlPool, errors::AppError};
use super::mssql_util::*;

pub struct OverviewRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub machine_table: String,
}

impl<'a> OverviewRepo<'a> {
    pub fn new(pool: &'a MssqlPool, machine_table: impl Into<String>) -> Self {
        Self { pool, machine_table: machine_table.into() }
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

        let kpi = kpi_rows.first().map(|r| {
            let total = i32_val(r, "total_key_machines");
            let down   = i32_val(r, "down_count");
            let wait   = i32_val(r, "waiting_count");
            let onproc = i32_val(r, "on_process_count");
            let closed = i32_val(r, "closed_this_shift");
            json!({
                "total_machines":   total,
                "running":          0.max(total - down - wait - onproc),
                "down":             down,
                "waiting":          wait,
                "on_process":       onproc,
                "closed_this_shift":closed,
            })
        }).unwrap_or_else(|| json!({ "total_machines": 0 }));

        let matrix: Vec<Value> = mat_rows.iter().map(|r| json!({
            "job_type":   str_val(r, "job_type"),
            "waiting":    i32_val(r, "waiting"),
            "on_process": i32_val(r, "on_process"),
            "closed":     i32_val(r, "closed"),
            "total":      i32_val(r, "total"),
        })).collect();

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
        Ok(rows.iter().map(|r| json!({
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
        })).collect())
    }
}
