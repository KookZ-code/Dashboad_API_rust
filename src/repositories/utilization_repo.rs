use serde_json::{json, Value};

use crate::{db::MssqlPool, errors::AppError, helpers::{kpi::*, where_builder::*},
            oracle::{OracleCache, agg::*}};
use super::mssql_util::*;

pub struct UtilizationRepo<'a> {
    pub pool: &'a MssqlPool,
    pub view: String,
    pub oracle: &'a OracleCache,
}

impl<'a> UtilizationRepo<'a> {
    pub fn new(pool: &'a MssqlPool, view: impl Into<String>, oracle: &'a OracleCache) -> Self {
        Self { pool, view: view.into(), oracle }
    }

    pub async fn detail(
        &self,
        start: Option<&str>, end: Option<&str>,
        areas: Option<&str>, shift: Option<&str>,
    ) -> Result<Value, AppError> {
        let wc = build_where(WhereOpts { start, end, areas, shift, machine_id: None, drill_day: None });
        let v = &self.view;
        let nd = n_days(start, end);
        let p = &wc.params;

        let down_in  = DOWN_TYPES.iter().map(|j| format!("'{}'", j)).collect::<Vec<_>>().join(", ");
        let lost_in  = LOST_TYPES.iter().map(|j| format!("'{}'", j)).collect::<Vec<_>>().join(", ");

        let sql_kpi = format!(
            "SELECT [{C_JT}] AS job_type, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS total_min, \
             SUM(ISNULL(Waiting_time,0)) AS wait_min \
             FROM {v} {} GROUP BY [{C_JT}]", wc.sql);
        let sql_month = format!(
            "SELECT CONVERT(VARCHAR(7),[{C_OPR}],120) AS ym, [{C_JT}] AS job_type, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS total_min, \
             SUM(ISNULL(Waiting_time,0)) AS wait_min \
             FROM {v} {} GROUP BY CONVERT(VARCHAR(7),[{C_OPR}],120),[{C_JT}] ORDER BY ym", wc.sql);
        let sql_area = format!(
            "SELECT [{C_AREA}] AS area, [{C_JT}] AS job_type, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS total_min, \
             SUM(ISNULL(Waiting_time,0)) AS wait_min \
             FROM {v} {} GROUP BY [{C_AREA}],[{C_JT}]", wc.sql);
        let sql_mc_cnt = format!(
            "SELECT [{C_AREA}] AS area, COUNT(DISTINCT [{C_MID}]) AS machine_count \
             FROM {v} {} GROUP BY [{C_AREA}]", wc.sql);
        let sql_total_mc = format!(
            "SELECT COUNT(DISTINCT [{C_MID}]) AS machine_count FROM {v} {}", wc.sql);
        let sql_scatter = format!(
            "SELECT [{C_MID}] AS machine_id, [{C_AREA}] AS area, COUNT(*) AS freq, \
             ROUND(AVG(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]))*1.0,1) AS avg_dur_min, \
             ROUND(CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0,1) AS total_hours \
             FROM {v} {} AND [{C_JT}] = 'M/C DOWN' \
             GROUP BY [{C_MID}],[{C_AREA}] HAVING COUNT(*) >= 2 ORDER BY total_hours DESC", wc.sql);
        let sql_top_down = format!(
            "SELECT TOP 10 [{C_SYM}] AS reason, \
             CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0 AS hours, COUNT(*) AS events \
             FROM {v} {} AND [{C_JT}] IN ({down_in}) \
             AND [{C_SYM}] IS NOT NULL AND [{C_SYM}] != '' \
             GROUP BY [{C_SYM}] ORDER BY hours DESC", wc.sql);
        let sql_top_lost = format!(
            "SELECT TOP 10 [{C_SYM}] AS reason, \
             CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0 AS hours, COUNT(*) AS events \
             FROM {v} {} AND [{C_JT}] IN ({lost_in}) \
             AND [{C_SYM}] IS NOT NULL AND [{C_SYM}] != '' \
             GROUP BY [{C_SYM}] ORDER BY hours DESC", wc.sql);

        let (r_kpi, r_month, r_area, r_mc_cnt, r_total_mc, r_scatter, r_top_down, r_top_lost) =
            tokio::try_join!(
                exec(self.pool, &sql_kpi,      p),
                exec(self.pool, &sql_month,    p),
                exec(self.pool, &sql_area,     p),
                exec(self.pool, &sql_mc_cnt,   p),
                exec(self.pool, &sql_total_mc, p),
                exec(self.pool, &sql_scatter,  p),
                exec(self.pool, &sql_top_down, p),
                exec(self.pool, &sql_top_lost, p),
            )?;

        // ── MSSQL typed rows ──
        let mut mc_count = r_total_mc.first().map(|r| i64_val(r, "machine_count")).unwrap_or(0);
        let mut kpi_rows: Vec<KpiRow> = r_kpi.iter().map(|r| KpiRow {
            job_type:  str_val(r, "job_type"),
            total_min: f64_val(r, "total_min"),
            wait_min:  f64_val(r, "wait_min"),
        }).collect();
        let mut month_rows: Vec<MonthlyRow> = r_month.iter().map(|r| MonthlyRow {
            ym:        str_val(r, "ym"),
            job_type:  str_val(r, "job_type"),
            total_min: f64_val(r, "total_min"),
            wait_min:  f64_val(r, "wait_min"),
        }).collect();
        let mut area_rows: Vec<AreaRow> = r_area.iter().map(|r| AreaRow {
            area:      str_val(r, "area"),
            job_type:  str_val(r, "job_type"),
            total_min: f64_val(r, "total_min"),
            wait_min:  f64_val(r, "wait_min"),
        }).collect();
        let mut mc_cnt_rows: Vec<McCountRow> = r_mc_cnt.iter().map(|r| McCountRow {
            area:          str_val(r, "area"),
            machine_count: i64_val(r, "machine_count"),
        }).collect();
        let mut scatter_rows: Vec<ScatterRow> = r_scatter.iter().map(|r| ScatterRow {
            machine_id:  str_val(r, "machine_id"),
            area:        str_val(r, "area"),
            freq:        i32_val(r, "freq") as i64,
            avg_dur_min: f64_val(r, "avg_dur_min"),
            total_hours: f64_val(r, "total_hours"),
        }).collect();

        // ── Oracle merge (ISO/FS) — extend vecs; kpi.rs helpers group+sum ──
        let mut oracle_rows: Vec<crate::oracle::model::OracleRow> = Vec::new();
        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            let ora = self.oracle.filter_historical(area_vec.as_deref(), start, end, shift);
            if !ora.is_empty() {
                kpi_rows.extend(ora_kpi_totals(&ora));
                month_rows.extend(ora_monthly(&ora));
                area_rows.extend(ora_by_area(&ora));
                mc_cnt_rows.extend(ora_machine_count(&ora));
                mc_count += ora_total_machine_count(&ora);
                scatter_rows.extend(ora_freq_vs_duration(&ora));
                oracle_rows = ora;
            }
        }

        let kpis = compute_kpis(&kpi_rows, mc_count, nd, shift);
        let monthly = monthly_trend(&month_rows, mc_count, shift);
        let area_totals = area_util(&area_rows, &mc_cnt_rows, nd, shift);

        let top_causes = |rows: &[tiberius::Row], oracle_filters: &[&str]| -> Value {
            let mut agg: std::collections::HashMap<String, (f64, i32)> = Default::default();

            // Aggregate SQL rows
            for r in rows {
                let reason = str_val(r, "reason");
                let h = f64_val(r, "hours");
                let e = i32_val(r, "events");
                let e_entry = agg.entry(reason).or_insert((0.0, 0));
                e_entry.0 += h;
                e_entry.1 += e;
            }

            // Aggregate Oracle rows by symptom
            for ora_r in &oracle_rows {
                if oracle_filters.contains(&ora_r.job_type.as_str()) && !ora_r.symptom.is_empty() {
                    let e = agg.entry(ora_r.symptom.clone()).or_insert((0.0, 0));
                    e.0 += ora_r.repair_min / 60.0;
                    e.1 += 1;
                }
            }

            let mut reasons: Vec<(String, f64, i32)> = agg.into_iter().map(|(k, (h, e))| (k, h, e)).collect();
            reasons.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let total: f64 = reasons.iter().map(|(_, h, _)| h).sum::<f64>().max(1.0);
            let mut cum = 0.0;
            let v: Vec<Value> = reasons.iter().take(10).map(|(reason, h, e)| {
                cum += h;
                json!({
                    "reason":         reason,
                    "hours":          r2(*h),
                    "events":         e,
                    "cumulative_pct": r2(cum / total * 100.0),
                })
            }).collect();
            json!(v)
        };

        scatter_rows.sort_by(|a, b| b.total_hours.partial_cmp(&a.total_hours).unwrap_or(std::cmp::Ordering::Equal));
        let scatter: Vec<Value> = scatter_rows.iter().map(|r| json!({
            "code_machine":   r.machine_id,
            "area":           r.area,
            "frequency":      r.freq,
            "avg_duration_h": r2(r.avg_dur_min / 60.0),
        })).collect();

        Ok(json!({
            "raw": {
                "kpi_totals": [
                    { "label": "utilization", "minutes": 0, "pct": kpis.utilization_pct },
                    { "label": "down",        "minutes": 0, "pct": kpis.downtime_pct },
                    { "label": "pm",          "minutes": 0, "pct": kpis.pm_pct },
                    { "label": "lost",        "minutes": 0, "pct": kpis.lost_time_pct },
                ],
                "area_totals": area_totals.iter().map(|a| json!({
                    "area": a.area, "utilization_pct": a.utilization_pct, "target_pct": a.target_pct
                })).collect::<Vec<_>>(),
                "machine_count": mc_count,
                "area_counts": mc_cnt_rows.iter().map(|a| json!({
                    "area": a.area, "machine_count": a.machine_count
                })).collect::<Vec<_>>(),
            },
            "monthly_trend": monthly.iter().map(|m| json!({
                "month": m.month, "running_min": m.running_min,
                "down_min": m.down_min, "pm_min": m.pm_min, "lost_min": m.lost_min,
            })).collect::<Vec<_>>(),
            "scatter": scatter,
            "top_down":  top_causes(&r_top_down, DOWN_TYPES),
            "top_lost":  top_causes(&r_top_lost, LOST_TYPES),
        }))
    }

    pub async fn by_machine(
        &self,
        start: Option<&str>, end: Option<&str>,
        areas: Option<&str>, shift: Option<&str>,
    ) -> Result<Vec<Value>, AppError> {
        let wc = build_where(WhereOpts { start, end, areas, shift, machine_id: None, drill_day: None });
        let nd = n_days(start, end);
        let h = match shift.map(|s| s.to_uppercase()).as_deref() {
            Some("DAY") | Some("NIGHT") => 12.0,
            _ => 24.0,
        };
        let sql = format!(
            "SELECT [{C_MID}] AS machine_id, [{C_AREA}] AS area, [{C_JT}] AS job_type, \
             SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS total_min, \
             SUM(ISNULL(Waiting_time,0)) AS wait_min \
             FROM {} {} GROUP BY [{C_MID}],[{C_AREA}],[{C_JT}]",
            self.view, wc.sql
        );
        let rows = exec(self.pool, &sql, &wc.params).await?;

        let mut map: std::collections::HashMap<String, (String, f64, f64, f64, f64)> = Default::default();
        for r in &rows {
            let mid = str_val(r, "machine_id");
            let area = str_val(r, "area");
            let jt   = str_val(r, "job_type");
            let tot  = f64_val(r, "total_min");
            let wt   = f64_val(r, "wait_min");
            let e = map.entry(mid).or_insert((area, 0.0, 0.0, 0.0, 0.0));
            if jt == "M/C DOWN"       { e.1 += tot; }
            else if jt == "PM"        { e.2 += tot; }
            else                       { e.3 += tot; }
            e.4 += wt;
        }

        // ── Oracle merge (ISO/FS) ──
        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            let ora = self.oracle.filter_historical(area_vec.as_deref(), start, end, shift);
            for r in ora {
                let mid = r.machine_id.clone();
                let e = map.entry(mid).or_insert((r.area.clone(), 0.0, 0.0, 0.0, 0.0));
                let tot = r.repair_min as f64;
                let wt = r.wait_min as f64;
                if r.job_type == "M/C DOWN"  { e.1 += tot; }
                else if r.job_type == "PM"   { e.2 += tot; }
                else                          { e.3 += tot; }
                e.4 += wt;
            }
        }

        let mut result: Vec<Value> = map.iter().map(|(mid, (area, dn, pm, ls, wt))| {
            let avail = (nd as f64 * h * 60.0).max(1.0);
            let used = dn + pm + ls + wt;
            json!({
                "code_machine":    mid,
                "area":            area,
                "utilization_pct": r2((100.0 - used / avail * 100.0).max(0.0)),
                "down_min":        dn,
                "pm_min":          pm,
                "lost_min":        ls,
            })
        }).collect();
        result.sort_by(|a, b| {
            let ua = a["utilization_pct"].as_f64().unwrap_or(0.0);
            let ub = b["utilization_pct"].as_f64().unwrap_or(0.0);
            ua.partial_cmp(&ub).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(result)
    }

    pub async fn attention(
        &self,
        start: Option<&str>, end: Option<&str>,
        areas: Option<&str>, shift: Option<&str>,
    ) -> Result<Vec<Value>, AppError> {
        let wc = build_where(WhereOpts { start, end, areas, shift, machine_id: None, drill_day: None });
        let sql = format!(
            "SELECT TOP 10 [{C_MID}] AS machine_id, [{C_AREA}] AS area, \
             ROUND(CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0,1) AS down_hours, \
             COUNT(*) AS event_count, \
             ROUND(AVG(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]))*1.0,0) AS avg_mttr_min, \
             ROUND(CAST(SUM(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}])) AS FLOAT)/60.0*2.0+COUNT(*) \
               +AVG(DATEDIFF(MINUTE,[{C_TECH}],[{C_END}]))/10.0,1) AS score \
             FROM {} {} AND [{C_JT}] = 'M/C DOWN' \
             GROUP BY [{C_MID}],[{C_AREA}] ORDER BY score DESC",
            self.view, wc.sql
        );
        let rows = exec(self.pool, &sql, &wc.params).await?;

        // ── Oracle merge (ISO/FS, M/C DOWN only) ──
        let mut all_rows: Vec<Value> = rows.iter().map(|r| json!({
            "machine_id":   str_val(r, "machine_id"),
            "area":         str_val(r, "area"),
            "down_hours":   f64_val(r, "down_hours"),
            "event_count":  i32_val(r, "event_count"),
            "avg_mttr_min": f64_val(r, "avg_mttr_min"),
            "score":        f64_val(r, "score"),
        })).collect();

        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            let ora = self.oracle.filter_historical(area_vec.as_deref(), start, end, shift);
            let mut oracle_agg: std::collections::HashMap<String, (String, f64, i32)> = Default::default();
            for r in ora.iter().filter(|r| r.job_type == "M/C DOWN") {
                let key = r.machine_id.clone();
                let e = oracle_agg.entry(key).or_insert((r.area.clone(), 0.0, 0));
                e.1 += r.repair_min as f64 / 60.0;
                e.2 += 1;
            }
            for (mid, (area, down_hours, event_count)) in oracle_agg {
                let avg_mttr_min = if event_count > 0 { (down_hours * 60.0 / event_count as f64).round() as f64 } else { 0.0 };
                let score = down_hours * 2.0 + event_count as f64 + avg_mttr_min / 10.0;
                all_rows.push(json!({
                    "machine_id":   mid,
                    "area":         area,
                    "down_hours":   r2(down_hours),
                    "event_count":  event_count,
                    "avg_mttr_min": r2(avg_mttr_min),
                    "score":        r2(score),
                }));
            }
        }

        all_rows.sort_by(|a, b| {
            let sa = a["score"].as_f64().unwrap_or(0.0);
            let sb = b["score"].as_f64().unwrap_or(0.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(all_rows.into_iter().take(10).collect())
    }
}
