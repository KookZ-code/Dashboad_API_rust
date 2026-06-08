use serde_json::{json, Value};
use std::collections::HashMap;

use crate::{db::MssqlPool, errors::AppError, helpers::{kpi::r2, where_builder::*}};
use super::mssql_util::*;

pub struct DowntimeDetailOpts<'a> {
    pub job_types:  Option<&'a str>,
    pub start:      Option<&'a str>,
    pub end:        Option<&'a str>,
    pub areas:      Option<&'a str>,
    pub shift:      Option<&'a str>,
    pub reason_col: Option<&'a str>,
    pub limit:      u32,
}

pub struct DowntimeEventOpts<'a> {
    pub job_types: Option<&'a str>,
    pub start:     Option<&'a str>,
    pub end:       Option<&'a str>,
    pub areas:     Option<&'a str>,
    pub shift:     Option<&'a str>,
    pub machine:   Option<&'a str>,
    pub symptom:   Option<&'a str>,
    pub cause:     Option<&'a str>,
    pub tech:      Option<&'a str>,
    pub limit:     u32,
}

pub struct DowntimeRepo<'a> {
    pub pool: &'a MssqlPool,
    pub view: String,
}

impl<'a> DowntimeRepo<'a> {
    pub fn new(pool: &'a MssqlPool, view: impl Into<String>) -> Self {
        Self { pool, view: view.into() }
    }

    pub async fn detail(&self, opts: DowntimeDetailOpts<'_>) -> Result<Value, AppError> {
        let DowntimeDetailOpts { job_types, start, end, areas, shift, reason_col, limit } = opts;
        let wc = build_where(WhereOpts { start, end, areas, shift, machine_id: None });
        let v = &self.view;
        let reason_c = if reason_col == Some("des_job") { C_SYM } else { C_CAUSE };
        let jt_list: Vec<&str> = match job_types {
            Some(s) if !s.is_empty() => s.split(',').map(|j| j.trim()).filter(|j| !j.is_empty()).collect(),
            _ => DOWN_TYPES.to_vec(),
        };
        let jt_in = jt_list.iter().map(|j| format!("'{}'", j)).collect::<Vec<_>>().join(", ");

        let sql_reason = format!(
            "SELECT [{reason_c}] AS reason, COUNT(*) AS cnt, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]))/60.0 AS hours, \
             AVG(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS avg_repair_min \
             FROM {v} {} AND [{C_JT}] IN ({jt_in}) \
             AND [{reason_c}] IS NOT NULL AND [{reason_c}] != '' \
             GROUP BY [{reason_c}] ORDER BY hours DESC", wc.sql);
        let sql_mr = format!(
            "SELECT [{C_MID}] AS code_machine, [{C_AREA}] AS area, \
             [{reason_c}] AS reason, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]))/60.0 AS hours \
             FROM {v} {} AND [{C_JT}] IN ({jt_in}) \
             AND [{reason_c}] IS NOT NULL AND [{reason_c}] != '' \
             GROUP BY [{C_MID}],[{C_AREA}],[{reason_c}]", wc.sql);
        let sql_shift = format!(
            "SELECT \
             CASE WHEN DATEPART(HOUR,[{C_OPR}]) >= 19 \
                  THEN CAST(DATEADD(DAY,1,CAST([{C_OPR}] AS DATE)) AS NVARCHAR(10)) \
                  ELSE CAST(CAST([{C_OPR}] AS DATE) AS NVARCHAR(10)) END AS day, \
             CASE WHEN DATEPART(HOUR,[{C_OPR}]) BETWEEN 7 AND 18 THEN 'Day' ELSE 'Night' END AS shift_name, \
             COUNT(*) AS events, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]))/60.0 AS repair_hrs, \
             SUM(ISNULL(Waiting_time,0))/60.0 AS wait_hrs \
             FROM {v} {} AND [{C_JT}] IN ({jt_in}) \
             GROUP BY \
               CASE WHEN DATEPART(HOUR,[{C_OPR}]) >= 19 \
                    THEN CAST(DATEADD(DAY,1,CAST([{C_OPR}] AS DATE)) AS NVARCHAR(10)) \
                    ELSE CAST(CAST([{C_OPR}] AS DATE) AS NVARCHAR(10)) END, \
               CASE WHEN DATEPART(HOUR,[{C_OPR}]) BETWEEN 7 AND 18 THEN 'Day' ELSE 'Night' END \
             ORDER BY day, shift_name", wc.sql);

        let p = &wc.params;
        let (r_reason, r_mr, r_shift) = tokio::try_join!(
            exec(self.pool, &sql_reason, p),
            exec(self.pool, &sql_mr, p),
            exec(self.pool, &sql_shift, p),
        )?;

        let total_hours:  f64 = r_reason.iter().map(|r| f64_val(r, "hours")).sum();
        let total_events: i64 = r_reason.iter().map(|r| i64_val(r, "cnt")).sum();
        let avg_repair_h = if total_events > 0 { r2(total_hours / total_events as f64) } else { 0.0 };
        let grand_total  = total_hours.max(1.0);
        let mut cum = 0.0;

        let lim = limit as usize;
        let reasons: Vec<Value> = r_reason.iter().take(lim).map(|r| {
            let h = f64_val(r, "hours");
            cum += h;
            json!({
                "reason":         str_val(r, "reason"),
                "count":          i64_val(r, "cnt"),
                "hours":          r2(h),
                "avg_repair_min": f64_val(r, "avg_repair_min"),
                "cumulative_pct": r2(cum / grand_total * 100.0),
            })
        }).collect();

        // Pivot machines by reason
        let mut pivot: HashMap<String, (String, String, f64, HashMap<String, f64>)> = Default::default();
        for r in &r_mr {
            let key = str_val(r, "code_machine");
            let e = pivot.entry(key.clone()).or_insert((
                key, str_val(r, "area"), 0.0, Default::default()
            ));
            let reason = str_val(r, "reason");
            let hours  = f64_val(r, "hours");
            *e.3.entry(reason).or_default() += hours;
            e.2 += hours;
        }
        let mut machines_by_reason: Vec<Value> = pivot.values().map(|(mid, area, total, reasons_map)| {
            let mut obj = serde_json::Map::new();
            obj.insert("code_machine".into(), json!(mid));
            obj.insert("area".into(), json!(area));
            obj.insert("total_hours".into(), json!(r2(*total)));
            for (k, v) in reasons_map { obj.insert(k.clone(), json!(r2(*v))); }
            Value::Object(obj)
        }).collect();
        machines_by_reason.sort_by(|a, b| {
            b["total_hours"].as_f64().unwrap_or(0.0)
                .partial_cmp(&a["total_hours"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        machines_by_reason.truncate(30);

        let top_machine   = machines_by_reason.first().and_then(|m| m["code_machine"].as_str()).unwrap_or("—").to_string();
        let top_machine_h = machines_by_reason.first().and_then(|m| m["total_hours"].as_f64()).unwrap_or(0.0);

        let daily_shift: Vec<Value> = r_shift.iter().map(|r| json!({
            "day":        str_val(r, "day"),
            "shift_name": str_val(r, "shift_name"),
            "events":     i32_val(r, "events"),
            "repair_hrs": f64_val(r, "repair_hrs"),
            "wait_hrs":   f64_val(r, "wait_hrs"),
        })).collect();

        Ok(json!({
            "reason":             reasons,
            "machines_by_reason": machines_by_reason,
            "daily_shift":        daily_shift,
            "kpi": {
                "total_hours":   r2(total_hours),
                "total_events":  total_events,
                "avg_repair_h":  avg_repair_h,
                "top_machine":   top_machine,
                "top_machine_h": r2(top_machine_h),
            },
        }))
    }

    pub async fn machines_with_downtime(&self, areas: Option<&str>) -> Result<Vec<String>, AppError> {
        let area_list: Vec<String> = areas
            .map(parse_areas)
            .unwrap_or_default();
        let mut clauses = vec![format!("[{}] = 'M/C DOWN'", C_JT)];
        let mut params: Vec<String> = Vec::new();

        if !area_list.is_empty() {
            let phs: Vec<String> = (1..=area_list.len()).map(|i| format!("@p{}", i)).collect();
            clauses.push(format!("[{}] IN ({})", C_AREA, phs.join(", ")));
            params.extend(area_list);
        }
        let sql = format!(
            "SELECT DISTINCT [{C_MID}] AS machine_id FROM {} WHERE {} ORDER BY [{C_MID}]",
            self.view, clauses.join(" AND ")
        );
        let rows = exec(self.pool, &sql, &params).await?;
        Ok(rows.iter().map(|r| str_val(r, "machine_id")).collect())
    }

    pub async fn events(&self, opts: DowntimeEventOpts<'_>) -> Result<Vec<Value>, AppError> {
        let DowntimeEventOpts { job_types, start, end, areas, shift, machine, symptom, cause, tech, limit } = opts;
        let lim = limit.clamp(50, 2000);
        let mut wc = build_where(WhereOpts { start, end, areas, shift, machine_id: None });
        let jt_default = "M/C DOWN,ENGINEERING DOWN,FACILITY DOWN,SETUP,SETUP BY OPERATOR,CONVERT,CLEAN MOLD,CHANGE CAP,PM";
        let jt_str = job_types.unwrap_or(jt_default);
        let jt_in = jt_str.split(',').map(|j| format!("'{}'", j.trim())).collect::<Vec<_>>().join(", ");
        let mut extra = format!(" AND [{C_JT}] IN ({jt_in})");

        if let Some(m) = machine {
            extra.push_str(&format!(" AND RTRIM(LTRIM([{C_MID}])) = @p{}", wc.params.len() + 1));
            wc.params.push(m.to_string());
        }
        if let Some(s) = symptom {
            extra.push_str(&format!(" AND [{C_SYM}] = @p{}", wc.params.len() + 1));
            wc.params.push(s.to_string());
        }
        if let Some(c) = cause {
            extra.push_str(&format!(" AND [{C_CAUSE}] = @p{}", wc.params.len() + 1));
            wc.params.push(c.to_string());
        }
        if let Some(t) = tech {
            extra.push_str(&format!(" AND ISNULL(by_perform,by_ack) = @p{}", wc.params.len() + 1));
            wc.params.push(t.to_string());
        }

        let sql = format!(
            "SELECT TOP {lim} [{C_OPR}] AS event_time, [{C_MID}] AS machine_id, \
             [{C_AREA}] AS area, [{C_JT}] AS job_type, [{C_SYM}] AS symptom, \
             [{C_CAUSE}] AS cause, ISNULL(by_perform,by_ack) AS tech, \
             ISNULL(Waiting_time,0) AS wait_min, \
             DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]) AS repair_min, \
             [mpc] AS die_mask, [lot_no] AS lot_no, [Package Type] AS package_type \
             FROM {} {} {extra} ORDER BY [{C_OPR}] DESC",
            self.view, wc.sql
        );
        let rows = exec(self.pool, &sql, &wc.params).await?;
        Ok(rows.iter().map(|r| json!({
            "event_time":   dt_str_or_empty(r, "event_time"),
            "machine_id":   str_val(r, "machine_id"),
            "area":         str_val(r, "area"),
            "job_type":     str_val(r, "job_type"),
            "symptom":      str_val(r, "symptom"),
            "cause":        str_val(r, "cause"),
            "tech":         str_val(r, "tech"),
            "wait_min":     i64_val(r, "wait_min"),
            "repair_min":   i64_val(r, "repair_min"),
            "die_mask":     str_val(r, "die_mask"),
            "lot_no":       str_val(r, "lot_no"),
            "package_type": str_val(r, "package_type"),
        })).collect())
    }
}
