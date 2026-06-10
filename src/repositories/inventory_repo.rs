use serde_json::{json, Value};

use crate::{db::MssqlPool, errors::AppError, helpers::where_builder::*};
use super::mssql_util::*;

pub struct InventoryRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub machine_table: String,
    pub view:          String,
}

impl<'a> InventoryRepo<'a> {
    pub fn new(pool: &'a MssqlPool, machine_table: impl Into<String>, view: impl Into<String>) -> Self {
        Self { pool, machine_table: machine_table.into(), view: view.into() }
    }

    pub async fn machines(&self, area: Option<&str>, key_only: bool) -> Result<Vec<Value>, AppError> {
        let sql = format!(
            "SELECT [code_machine],[des_machine],[mfg],[model],[sn],[id_operation],[short_name],[date_install], \
             [flag_key],[flag_automotive],[flag_gold] \
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
