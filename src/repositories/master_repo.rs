use serde_json::{json, Value};
use tiberius::Row;

use crate::{db::MssqlPool, errors::AppError};
use super::mssql_util::*;

pub struct MasterRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub machine_table: String,
    pub view:          String,
}

impl<'a> MasterRepo<'a> {
    pub fn new(pool: &'a MssqlPool, machine_table: impl Into<String>, view: impl Into<String>) -> Self {
        Self { pool, machine_table: machine_table.into(), view: view.into() }
    }

    pub async fn areas(&self) -> Result<Vec<Value>, AppError> {
        let sql = format!(
            "SELECT DISTINCT [id_operation] AS area, [short_name] \
             FROM {} WHERE [id_operation] IS NOT NULL AND [id_operation] != '' \
             ORDER BY [id_operation]",
            self.machine_table
        );
        let rows = exec(self.pool, &sql, &[]).await?;
        Ok(rows.iter().map(|r| json!({
            "area":       str_val(r, "area"),
            "short_name": str_val(r, "short_name"),
        })).collect())
    }

    pub async fn machines(&self, area: Option<&str>, key_only: bool) -> Result<Vec<Value>, AppError> {
        let mut clauses = vec!["ISNULL([flag_delete],0) != 1".to_string()];
        let mut params: Vec<String> = Vec::new();

        if let Some(a) = area {
            clauses.push(format!("[id_operation] = @p{}", params.len() + 1));
            params.push(a.to_string());
        }
        if key_only { clauses.push("[flag_key] = 1".to_string()); }

        let where_sql = format!("WHERE {}", clauses.join(" AND "));
        let mc_sql = format!(
            "SELECT [code_machine] AS machine_id, [des_machine], [id_operation] AS area, [short_name] AS area_name \
             FROM {} {} ORDER BY [id_operation],[code_machine]",
            self.machine_table, where_sql
        );
        let flag_sql = format!(
            "SELECT [code_machine],[flag_key],[flag_automotive],[flag_gold],[mfg],[model],[sn] \
             FROM {} WHERE [id_operation] IS NOT NULL AND ISNULL([flag_delete],0) != 1",
            self.machine_table
        );

        let (mc_rows, flag_rows) = tokio::try_join!(
            exec(self.pool, &mc_sql, &params),
            exec(self.pool, &flag_sql, &[]),
        )?;

        let flags: std::collections::HashMap<String, &Row> = flag_rows.iter()
            .map(|r| (str_val(r, "code_machine"), r))
            .collect();

        Ok(mc_rows.iter().map(|r| {
            let mid = str_val(r, "machine_id");
            let f = flags.get(&mid);
            json!({
                "machine_id":      mid,
                "des_machine":     str_val(r, "des_machine"),
                "area":            str_val(r, "area"),
                "area_name":       str_val(r, "area_name"),
                "mfg":             f.map(|ff| str_val(ff, "mfg")).unwrap_or_default(),
                "model":           f.map(|ff| str_val(ff, "model")).unwrap_or_default(),
                "sn":              f.map(|ff| str_val(ff, "sn")).unwrap_or_default(),
                "flag_key":        f.map(|ff| i32_val(ff, "flag_key")).unwrap_or(0),
                "flag_automotive": f.map(|ff| i32_val(ff, "flag_automotive")).unwrap_or(0),
                "flag_gold":       f.map(|ff| i32_val(ff, "flag_gold")).unwrap_or(0),
            })
        }).collect())
    }

    pub async fn machine_detail(&self, id: &str, recent_limit: u32) -> Result<Value, AppError> {
        let lim = recent_limit.clamp(1, 200);
        let info_sql = format!(
            "SELECT [code_machine],[des_machine],[mfg],[model],[sn],[id_operation] AS area,[flag_key],[remark],[date_install] \
             FROM {} WHERE RTRIM(LTRIM([code_machine])) = @p1",
            self.machine_table
        );
        let kpi_sql = format!(
            "SELECT COUNT(CASE WHEN [job_type]='M/C DOWN' THEN 1 END) AS down_events, \
             ROUND(AVG(CASE WHEN [job_type]='M/C DOWN' THEN DATEDIFF(MINUTE,[date_ack],[date_close])*1.0 END),0) AS avg_mttr_min, \
             COUNT(CASE WHEN [job_type]='PM' THEN 1 END) AS pm_events \
             FROM {} WHERE RTRIM(LTRIM([code_machine]))=@p1 \
               AND [datex] >= DATEADD(DAY,-30,GETDATE()) AND [date_ack] IS NOT NULL AND [date_close] > [date_ack]",
            self.view
        );
        let ev_sql = format!(
            "SELECT TOP {} [datex] AS ts,[job_type], \
             CASE WHEN [date_ack] IS NULL THEN 'Waiting' WHEN [date_close] IS NULL THEN 'On Process' ELSE 'Closed' END AS status, \
             DATEDIFF(MINUTE,COALESCE([date_ack],[datex]),COALESCE([date_close],GETDATE())) AS duration_min \
             FROM {} WHERE RTRIM(LTRIM([code_machine]))=@p1 ORDER BY [datex] DESC",
            lim, self.view
        );

        let params = vec![id.to_string()];
        let (info_rows, kpi_rows, ev_rows) = tokio::try_join!(
            exec(self.pool, &info_sql, &params),
            exec(self.pool, &kpi_sql, &params),
            exec(self.pool, &ev_sql, &params),
        )?;

        let info = info_rows.first().map(|r| json!({
            "machine_id":  str_val(r, "code_machine"),
            "des_machine": str_val(r, "des_machine"),
            "area":        str_val(r, "area"),
            "flag_key":    i32_val(r, "flag_key"),
            "mfg":         str_val(r, "mfg"),
            "model":       str_val(r, "model"),
            "sn":          str_val(r, "sn"),
            "notes":       opt_str(r, "remark"),
        })).unwrap_or_else(|| json!({ "machine_id": id, "area": "", "flag_key": 0 }));

        let kpis = kpi_rows.first().map(|r| json!({
            "down_events":  i32_val(r, "down_events"),
            "avg_mttr_min": f64_val(r, "avg_mttr_min"),
            "pm_events":    i32_val(r, "pm_events"),
        })).unwrap_or_else(|| json!({ "down_events": 0, "avg_mttr_min": 0, "pm_events": 0 }));

        let events: Vec<Value> = ev_rows.iter().map(|r| json!({
            "ts":           dt_str_or_empty(r, "ts"),
            "job_type":     str_val(r, "job_type"),
            "status":       str_val(r, "status"),
            "duration_min": i64_val(r, "duration_min"),
        })).collect();

        Ok(json!({ "info": info, "kpis": kpis, "recent_events": events }))
    }

    pub async fn machine_records(&self, id: &str, limit: u32) -> Result<Vec<Value>, AppError> {
        let lim = limit.clamp(1, 1000);
        let sql = format!(
            "SELECT TOP {} * FROM {} WHERE RTRIM(LTRIM([code_machine]))=@p1 ORDER BY [datex] DESC",
            lim, self.view
        );
        let rows = exec(self.pool, &sql, &[id.to_string()]).await?;
        Ok(rows.iter().map(|r| {
            // Convert all columns to JSON — datetime fields formatted as string
            let mut map = serde_json::Map::new();
            for col in r.columns() {
                let name = col.name().to_string();
                let val: Value = r.get::<chrono::NaiveDateTime, _>(name.as_str())
                    .map(|dt| Value::String(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()))
                    .or_else(|| r.get::<f64, _>(name.as_str()).map(|v| json!(v)))
                    .or_else(|| r.get::<i64, _>(name.as_str()).map(|v| json!(v)))
                    .or_else(|| r.get::<i32, _>(name.as_str()).map(|v| json!(v)))
                    .or_else(|| r.get::<&str, _>(name.as_str()).map(|v| Value::String(v.to_string())))
                    .unwrap_or(Value::Null);
                map.insert(name, val);
            }
            Value::Object(map)
        }).collect())
    }
}
