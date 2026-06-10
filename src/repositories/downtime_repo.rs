use serde_json::{json, Value};
use std::collections::HashMap;

use crate::{db::MssqlPool, errors::AppError, helpers::{kpi::r2, where_builder::*},
            oracle::{OracleCache, agg::*}};
use super::mssql_util::*;

pub struct DowntimeDetailOpts<'a> {
    pub job_types:  Option<&'a str>,
    pub start:      Option<&'a str>,
    pub end:        Option<&'a str>,
    pub areas:      Option<&'a str>,
    pub shift:      Option<&'a str>,
    pub reason_col: Option<&'a str>,
    pub limit:      u32,
    pub drill_day:  Option<&'a str>,
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
    pub drill_day: Option<&'a str>,
}

pub struct DowntimeRepo<'a> {
    pub pool:   &'a MssqlPool,
    pub view:   String,
    pub oracle: &'a OracleCache,
}

impl<'a> DowntimeRepo<'a> {
    pub fn new(pool: &'a MssqlPool, view: impl Into<String>, oracle: &'a OracleCache) -> Self {
        Self { pool, view: view.into(), oracle }
    }

    pub async fn detail(&self, opts: DowntimeDetailOpts<'_>) -> Result<Value, AppError> {
        let DowntimeDetailOpts { job_types, start, end, areas, shift, reason_col, limit, drill_day } = opts;

        let (sql_areas, ora_areas) = partition_areas(areas);
        let use_sql = areas.is_none() || !sql_areas.is_empty();
        let use_ora = self.oracle.enabled && (areas.is_none() || !ora_areas.is_empty());

        let sql_areas_str: Option<String> =
            if sql_areas.is_empty() { None } else { Some(sql_areas.join(",")) };
        let wc = build_where(WhereOpts {
            start, end, areas: sql_areas_str.as_deref(), shift, machine_id: None, drill_day,
        });

        let v = &self.view;
        let reason_c = if reason_col == Some("des_job") { C_SYM } else { C_CAUSE };
        let jt_list: Vec<&str> = match job_types {
            Some(s) if !s.is_empty() => s.split(',').map(|j| j.trim()).filter(|j| !j.is_empty()).collect(),
            _ => DOWN_TYPES.to_vec(),
        };
        let jt_in = jt_list.iter().map(|j| format!("'{}'", j)).collect::<Vec<_>>().join(", ");

        // ── SQL Server queries (skipped when only Oracle areas requested) ─────
        let mut reason_map: HashMap<String, (i64, f64)> = HashMap::new();
        let mut mr_rows:    Vec<(String, String, String, f64)> = Vec::new();
        let mut daily_map:  HashMap<(String, String), (i64, f64, f64)> = HashMap::new();

        if use_sql {
            let sql_reason = format!(
                "SELECT [{reason_c}] AS reason, COUNT(*) AS cnt, \
                 CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0 AS hours, \
                 AVG(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS avg_repair_min \
                 FROM {v} {} AND [{C_JT}] IN ({jt_in}) \
                 AND [{reason_c}] IS NOT NULL AND [{reason_c}] != '' \
                 GROUP BY [{reason_c}] ORDER BY hours DESC", wc.sql);
            let sql_mr = format!(
                "SELECT [{C_MID}] AS code_machine, [{C_AREA}] AS area, \
                 [{reason_c}] AS reason, \
                 CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0 AS hours \
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
                 CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0 AS repair_hrs, \
                 CAST(SUM(ISNULL(Waiting_time,0)) AS FLOAT)/60.0 AS wait_hrs \
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
                exec(self.pool, &sql_mr,     p),
                exec(self.pool, &sql_shift,  p),
            )?;

            for r in &r_reason {
                let e = reason_map.entry(str_val(r, "reason")).or_insert((0, 0.0));
                e.0 += i64_val(r, "cnt");
                e.1 += f64_val(r, "hours") * 60.0;
            }
            for r in &r_mr {
                mr_rows.push((str_val(r, "code_machine"), str_val(r, "area"), str_val(r, "reason"), f64_val(r, "hours")));
            }
            for r in &r_shift {
                let e = daily_map.entry((str_val(r, "day"), str_val(r, "shift_name"))).or_insert((0, 0.0, 0.0));
                e.0 += i32_val(r, "events") as i64;
                e.1 += f64_val(r, "repair_hrs") * 60.0;
                e.2 += f64_val(r, "wait_hrs")   * 60.0;
            }
        }

        // ── Oracle (ISO/FS only, skipped when no Oracle areas requested) ──────
        // Oracle reason always uses CRITERIA (symptom) — CAUSE is a constant process
        // descriptor ("Trim Form-Isolate Down") that yields one useless Pareto bar.
        if use_ora {
            let ora_filter: Option<&[String]> = if areas.is_none() { None } else { Some(&ora_areas) };
            let ora_raw = self.oracle.filter_historical(ora_filter, start, end, shift);
            // Oracle drill_day: match datex.date() directly — Oracle SHIFT column (D/N)
            // is the authoritative shift marker, so no cross-midnight offset needed.
            let ora: Vec<_> = if let Some(dd) = drill_day {
                ora_raw.into_iter()
                    .filter(|r| r.datex.map(|dt| dt.date().format("%Y-%m-%d").to_string() == dd).unwrap_or(false))
                    .collect()
            } else { ora_raw };
            if !ora.is_empty() {
                for d in ora_dt_reason(&ora, &jt_list, true) {
                    let e = reason_map.entry(d.reason).or_insert((0, 0.0));
                    e.0 += d.cnt; e.1 += d.sum_min;
                }
                for m in ora_dt_machine(&ora, &jt_list, true) {
                    mr_rows.push((m.machine_id, m.area, m.reason, m.hours));
                }
                for d in ora_dt_daily(&ora, &jt_list) {
                    let e = daily_map.entry((d.day, d.shift_name)).or_insert((0, 0.0, 0.0));
                    e.0 += d.events; e.1 += d.repair_min; e.2 += d.wait_min;
                }
            }
        }

        // ── Reasons (sorted by hours desc) + pareto ──────────────────────────
        let mut reason_vec: Vec<(String, i64, f64)> =
            reason_map.into_iter().map(|(r, (c, sm))| (r, c, sm)).collect();
        reason_vec.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        let total_hours:  f64 = reason_vec.iter().map(|(_, _, sm)| sm / 60.0).sum();
        let total_events: i64 = reason_vec.iter().map(|(_, c, _)| *c).sum();
        let grand_total  = total_hours.max(1.0);
        let mut cum = 0.0;
        let reasons: Vec<Value> = reason_vec.iter().take(limit as usize).map(|(reason, cnt, sm)| {
            let h = sm / 60.0;
            cum += h;
            json!({
                "reason":         reason,
                "count":          cnt,
                "hours":          r2(h),
                "avg_repair_min": if *cnt > 0 { r2(sm / *cnt as f64) } else { 0.0 },
                "cumulative_pct": r2(cum / grand_total * 100.0),
            })
        }).collect();

        // ── Machines pivot ────────────────────────────────────────────────────
        let mut pivot: HashMap<String, (String, String, f64, HashMap<String, f64>)> = Default::default();
        for (mid, area, reason, hours) in mr_rows {
            let e = pivot.entry(mid.clone()).or_insert((mid, area, 0.0, Default::default()));
            *e.3.entry(reason).or_default() += hours;
            e.2 += hours;
        }
        let mut machines_by_reason: Vec<Value> = pivot.values().map(|(mid, area, total, reasons_map)| {
            let mut obj = serde_json::Map::new();
            obj.insert("code_machine".into(), json!(mid));
            obj.insert("area".into(),         json!(area));
            obj.insert("total_hours".into(),  json!(r2(*total)));
            for (k, v) in reasons_map { obj.insert(k.clone(), json!(r2(*v))); }
            Value::Object(obj)
        }).collect();
        machines_by_reason.sort_by(|a, b|
            b["total_hours"].as_f64().unwrap_or(0.0)
                .partial_cmp(&a["total_hours"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal));
        machines_by_reason.truncate(30);

        // ── MTTR / MTTW from daily_map (includes all events, not just those with reasons) ──
        let daily_total_events: i64 = daily_map.values().map(|(e, _, _)| *e).sum();
        let daily_total_repair: f64 = daily_map.values().map(|(_, r, _)| *r).sum();
        let daily_total_wait:   f64 = daily_map.values().map(|(_, _, w)| *w).sum();
        let avg_repair_min = if daily_total_events > 0 { r2(daily_total_repair / daily_total_events as f64) } else { 0.0 };
        let avg_wait_min   = if daily_total_events > 0 { r2(daily_total_wait   / daily_total_events as f64) } else { 0.0 };

        let mut daily_vec: Vec<((String, String), (i64, f64, f64))> = daily_map.into_iter().collect();
        daily_vec.sort_by(|a, b| a.0.cmp(&b.0));
        let daily_shift: Vec<Value> = daily_vec.iter().map(|((day, shift_name), (events, rep, wait))| json!({
            "day":        day,
            "shift_name": shift_name,
            "events":     events,
            "repair_hrs": r2(rep / 60.0),
            "wait_hrs":   r2(wait / 60.0),
        })).collect();

        Ok(json!({
            "reason":             reasons,
            "machines_by_reason": machines_by_reason,
            "daily_shift":        daily_shift,
            "kpi": {
                "total_hours":    r2(total_hours),
                "total_events":   total_events,
                "avg_repair_min": avg_repair_min,
                "avg_wait_min":   avg_wait_min,
            },
        }))
    }

    pub async fn machines_with_downtime(&self, areas: Option<&str>) -> Result<Vec<String>, AppError> {
        let (sql_areas, _) = partition_areas(areas);
        let mut clauses = vec![format!("[{}] = 'M/C DOWN'", C_JT)];
        let mut params: Vec<String> = Vec::new();
        if !sql_areas.is_empty() {
            let phs: Vec<String> = (1..=sql_areas.len()).map(|i| format!("@p{}", i)).collect();
            clauses.push(format!("[{}] IN ({})", C_AREA, phs.join(", ")));
            params.extend(sql_areas);
        }
        let sql = format!(
            "SELECT DISTINCT [{C_MID}] AS machine_id FROM {} WHERE {} ORDER BY [{C_MID}]",
            self.view, clauses.join(" AND ")
        );
        let rows = exec(self.pool, &sql, &params).await?;
        Ok(rows.iter().map(|r| str_val(r, "machine_id")).collect())
    }

    pub async fn events(&self, opts: DowntimeEventOpts<'_>) -> Result<Vec<Value>, AppError> {
        let DowntimeEventOpts { job_types, start, end, areas, shift, machine, symptom, cause, tech, limit, drill_day } = opts;
        let lim = limit.clamp(50, 2000);

        let (sql_areas, ora_areas) = partition_areas(areas);
        let use_sql = areas.is_none() || !sql_areas.is_empty();
        let use_ora = self.oracle.enabled && (areas.is_none() || !ora_areas.is_empty());

        let sql_areas_str: Option<String> =
            if sql_areas.is_empty() { None } else { Some(sql_areas.join(",")) };
        let mut wc = build_where(WhereOpts {
            start, end, areas: sql_areas_str.as_deref(), shift, machine_id: None, drill_day,
        });

        let jt_default = "M/C DOWN,ENGINEERING DOWN,FACILITY DOWN,SETUP,SETUP BY OPERATOR,CONVERT,CLEAN MOLD,CHANGE CAP,PM";
        let jt_str = job_types.unwrap_or(jt_default);
        let jt_in  = jt_str.split(',').map(|j| format!("'{}'", j.trim())).collect::<Vec<_>>().join(", ");
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

        let mut events: Vec<Value> = Vec::new();

        // ── SQL Server (non-Oracle areas) ─────────────────────────────────────
        if use_sql {
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
            for r in &rows {
                events.push(json!({
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
                }));
            }
        }

        // ── Oracle (ISO/FS only) ──────────────────────────────────────────────
        if use_ora {
            let ora_filter: Option<&[String]> = if areas.is_none() { None } else { Some(&ora_areas) };
            let ora_raw = self.oracle.filter_historical(ora_filter, start, end, shift);
            let ora: Vec<_> = if let Some(dd) = drill_day {
                ora_raw.into_iter()
                    .filter(|r| r.datex.map(|dt| dt.date().format("%Y-%m-%d").to_string() == dd).unwrap_or(false))
                    .collect()
            } else { ora_raw };
            if !ora.is_empty() {
                let jt_list: Vec<&str> = jt_str.split(',').map(|j| j.trim()).filter(|j| !j.is_empty()).collect();
                for e in ora_dt_events(&ora, &jt_list, machine, symptom, cause, tech) {
                    events.push(json!({
                        "event_time":   e.event_time,
                        "machine_id":   e.machine_id,
                        "area":         e.area,
                        "job_type":     e.job_type,
                        "symptom":      e.symptom,
                        "cause":        e.cause,
                        "tech":         e.tech,
                        "wait_min":     e.wait_min,
                        "repair_min":   e.repair_min,
                        "die_mask":     e.die_mask,
                        "lot_no":       e.lot_no,
                        "package_type": e.package_type,
                    }));
                }
            }
        }

        events.sort_by(|a, b| b["event_time"].as_str().unwrap_or("").cmp(a["event_time"].as_str().unwrap_or("")));
        events.truncate(lim as usize);
        Ok(events)
    }
}
