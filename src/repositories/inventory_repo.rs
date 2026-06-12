use serde_json::{json, Value};

use crate::{db::MssqlPool, errors::AppError, helpers::where_builder::*};
use super::mssql_util::*;

pub struct InventoryRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub machine_table: String,
    pub view:          String,
    pub job_table:     String,
}

impl<'a> InventoryRepo<'a> {
    pub fn new(pool: &'a MssqlPool, machine_table: impl Into<String>, view: impl Into<String>, job_table: impl Into<String>) -> Self {
        Self { pool, machine_table: machine_table.into(), view: view.into(), job_table: job_table.into() }
    }

    pub async fn machines(&self, area: Option<&str>, key_only: bool) -> Result<Vec<Value>, AppError> {
        let sql = format!(
            "SELECT [code_machine],[des_machine],[mfg],[model],[sn],[id_operation],[short_name], \
             CONVERT(VARCHAR(10),[date_install],120) AS date_install, \
             CAST([flag_key] AS INT) AS flag_key, \
             CAST([flag_automotive] AS INT) AS flag_automotive, \
             CAST([flag_gold] AS INT) AS flag_gold \
             FROM {} WHERE [id_operation] IS NOT NULL AND [id_operation] != '' \
               AND ISNULL([flag_delete],0) != 1 ORDER BY [id_operation],[code_machine]",
            self.machine_table
        );
        let rows = exec(self.pool, &sql, &[]).await?;

        Ok(rows.iter()
            .filter(|r| area.is_none_or(|a| str_val(r, "id_operation") == a))
            .filter(|r| !key_only || i32_val(r, "flag_key") == 1)
            .map(|r| {
                let yr: Option<i32> = opt_str(r, "date_install")
                    .and_then(|s| s[..4.min(s.len())].parse::<i32>().ok());
                json!({
                    "machine_id":      str_val(r, "code_machine"),
                    "des_machine":     str_val(r, "des_machine"),
                    "area":            str_val(r, "id_operation"),
                    "area_name":       str_val(r, "short_name"),
                    "mfg":             str_val(r, "mfg"),
                    "model":           str_val(r, "model"),
                    "sn":              str_val(r, "sn"),
                    "flag_key":        i32_val(r, "flag_key"),
                    "flag_automotive": i32_val(r, "flag_automotive"),
                    "flag_gold":       i32_val(r, "flag_gold"),
                    "year_install":    yr,
                })
            })
            .collect())
    }

    /// Probe job_listx column names (used once to discover schema).
    pub async fn probe_job_columns(&self) -> Result<Vec<Value>, AppError> {
        let sql = "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS \
                   WHERE TABLE_CATALOG = 'MTHAI_ppm_db1' AND TABLE_SCHEMA = 'dbo' AND TABLE_NAME = 'job_listx' \
                   ORDER BY ORDINAL_POSITION";
        let rows = exec(self.pool, sql, &[]).await?;
        Ok(rows.iter().map(|r| json!({ "col": str_val(r, "COLUMN_NAME") })).collect())
    }

    /// Get the most recently run [Package Type] per machine from job_listx.
    /// Column name has a space so it is quoted with []. ROW_NUMBER deduplicates ties.
    pub async fn last_package(&self) -> Result<Vec<Value>, AppError> {
        let sql = format!(
            "SELECT code_machine, [Package Type] AS package_type, CONVERT(VARCHAR(10), datex, 120) AS last_run \
             FROM ( \
               SELECT code_machine, [Package Type], datex, \
                      ROW_NUMBER() OVER (PARTITION BY code_machine ORDER BY datex DESC) AS rn \
               FROM {} \
               WHERE code_machine IS NOT NULL AND code_machine != '' \
             ) r WHERE r.rn = 1",
            self.job_table
        );
        let rows = exec(self.pool, &sql, &[]).await?;
        Ok(rows.iter().map(|r| json!({
            "code_machine":  str_val(r, "code_machine"),
            "package_type":  str_val(r, "package_type"),
            "last_run":      opt_str(r, "last_run"),
        })).collect())
    }

    pub async fn downtime_summary(&self) -> Result<Vec<Value>, AppError> {
        let sql = format!(
            "SELECT [{C_MID}] AS code_machine, COUNT(*) AS down_events, \
             ROUND(CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0,1) AS down_hrs, \
             ROUND(AVG(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])),0) AS avg_mttr_min \
             FROM {} \
             WHERE [{C_JT}] = 'M/C DOWN' AND [{C_TECH}] IS NOT NULL AND [{C_END}] > [{C_TECH}] \
               AND [{C_OPR}] >= DATEADD(DAY,-7,GETDATE()) \
             GROUP BY [{C_MID}]",
            self.view
        );
        let rows = exec(self.pool, &sql, &[]).await?;
        Ok(rows.iter().map(|r| json!({
            "code_machine":  str_val(r, "code_machine"),
            "down_events":   i32_val(r, "down_events"),
            "down_hrs":      f64_val(r, "down_hrs"),
            "avg_mttr_min":  f64_val(r, "avg_mttr_min"),
        })).collect())
    }
}
