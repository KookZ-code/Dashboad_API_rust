use serde_json::{json, Value};
use std::collections::HashMap;

use crate::{db::MssqlPool, errors::AppError, helpers::where_builder::*,
            oracle::{OracleCache, agg::{ora_tech_metrics, TechMetric}}};
use super::mssql_util::*;

pub struct TechRepo<'a> {
    pub pool: &'a MssqlPool,
    pub view: String,
    pub oracle: &'a OracleCache,
}

impl<'a> TechRepo<'a> {
    pub fn new(pool: &'a MssqlPool, view: impl Into<String>, oracle: &'a OracleCache) -> Self {
        Self { pool, view: view.into(), oracle }
    }

    pub async fn metrics(
        &self,
        start: Option<&str>, end: Option<&str>,
        areas: Option<&str>, shift: Option<&str>,
        job_type: Option<&str>,
    ) -> Result<Vec<Value>, AppError> {
        let wc = build_tech_where(TechWhereOpts { start, end, areas, shift, job_type });
        let v = &self.view;
        // Use a single filtered CTE so params appear only once
        let sql = format!(
            "WITH filtered AS ( \
               SELECT ISNULL(by_perform, by_ack) AS technician, \
                      code_machine, id_operation, date_ack, date_close, datex, \
                      Waiting_time, job_type \
               FROM {v} {} \
             ), \
             base AS ( \
               SELECT technician, COUNT(*) AS job_count, \
                      ROUND(AVG(ISNULL(Waiting_time,0)*1.0),1) AS avg_response_min, \
                      ROUND(AVG(DATEDIFF(MINUTE,date_ack,date_close)*1.0),1) AS avg_repair_min, \
                      COUNT(DISTINCT id_operation) AS area_count \
               FROM filtered GROUP BY technician \
             ), \
             mc_jobs AS ( \
               SELECT technician, code_machine, date_close, \
                      LEAD(datex) OVER ( \
                        PARTITION BY technician, code_machine ORDER BY datex \
                      ) AS next_same_date \
               FROM filtered WHERE job_type = 'M/C DOWN' \
             ), \
             ftfr AS ( \
               SELECT technician, COUNT(*) AS mc_total, \
                      SUM(CASE WHEN next_same_date IS NULL \
                               OR DATEDIFF(DAY, date_close, next_same_date) > 7 \
                          THEN 1 ELSE 0 END) AS first_fixes \
               FROM mc_jobs GROUP BY technician \
             ) \
             SELECT b.technician, b.job_count, b.avg_response_min, b.avg_repair_min, b.area_count, \
                    COALESCE(ROUND(CAST(f.first_fixes AS FLOAT)/NULLIF(f.mc_total,0)*100,1),100) AS ftfr_pct \
             FROM base b LEFT JOIN ftfr f ON b.technician = f.technician \
             ORDER BY b.job_count DESC",
            wc.sql
        );

        let rows = exec(self.pool, &sql, &wc.params).await?;

        // MSSQL per-tech metrics
        let mut metrics: Vec<TechMetric> = rows.iter().map(|r| TechMetric {
            technician:       str_val(r, "technician"),
            job_count:        f64_val(r, "job_count"),
            avg_response_min: f64_val(r, "avg_response_min"),
            avg_repair_min:   f64_val(r, "avg_repair_min"),
            area_count:       i32_val(r, "area_count") as i64,
            ftfr_pct:         f64_val(r, "ftfr_pct"),
        }).collect();

        // ── Oracle merge (ISO/FS) ──
        if self.oracle.enabled {
            let area_vec: Option<Vec<String>> = areas.map(parse_areas);
            let mut ora = self.oracle.filter_historical(area_vec.as_deref(), start, end, shift);
            if let Some(jt) = job_type { ora.retain(|r| r.job_type == jt); }
            metrics.extend(ora_tech_metrics(&ora));
        }

        if metrics.is_empty() { return Ok(vec![]); }

        // Re-aggregate by technician (a tech may appear in both SQL + Oracle):
        // job_count sum · response/repair/ftfr = mean of the per-source averages · area_count max
        struct G { jobs: f64, resp: Vec<f64>, repair: Vec<f64>, area: i64, ftfr: Vec<f64> }
        let mut g: HashMap<String, G> = HashMap::new();
        for m in metrics {
            let e = g.entry(m.technician).or_insert(G { jobs: 0.0, resp: vec![], repair: vec![], area: 0, ftfr: vec![] });
            e.jobs += m.job_count;
            e.resp.push(m.avg_response_min);
            e.repair.push(m.avg_repair_min);
            e.area = e.area.max(m.area_count);
            e.ftfr.push(m.ftfr_pct);
        }
        let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
        let merged: Vec<TechMetric> = g.into_iter().map(|(technician, e)| TechMetric {
            technician,
            job_count:        e.jobs,
            avg_response_min: mean(&e.resp),
            avg_repair_min:   mean(&e.repair),
            area_count:       e.area,
            ftfr_pct:         mean(&e.ftfr),
        }).collect();

        let avg_jobs = (merged.iter().map(|m| m.job_count).sum::<f64>() / merged.len() as f64).max(1.0);
        let mut out: Vec<Value> = merged.iter().map(|m| tech_score(m, avg_jobs)).collect();
        out.sort_by(|a, b| b["job_count"].as_i64().unwrap_or(0).cmp(&a["job_count"].as_i64().unwrap_or(0)));
        Ok(out)
    }

    pub async fn list(&self) -> Result<Vec<Value>, AppError> {
        let rows = exec(
            self.pool,
            "SELECT [Badge],[Name],[NameTH],[AERA],[Job Desc],[Supv],[Group] FROM dbo.TechnicianList",
            &[],
        ).await?;
        Ok(rows.iter().map(|r| json!({
            "Badge":    str_val(r, "Badge"),
            "Name":     str_val(r, "Name"),
            "NameTH":   str_val(r, "NameTH"),
            "AERA":     str_val(r, "AERA"),
            "Job Desc": str_val(r, "Job Desc"),
            "Supv":     str_val(r, "Supv"),
            "Group":    str_val(r, "Group"),
        })).collect())
    }
}

fn norm(val: f64, lo: f64, hi: f64, invert: bool) -> f64 {
    if (hi - lo).abs() < f64::EPSILON { return 50.0; }
    let n = ((val - lo) / (hi - lo)).clamp(0.0, 1.0);
    ((if invert { 1.0 - n } else { n }) * 1000.0).round() / 10.0
}

fn tech_score(m: &TechMetric, avg_jobs: f64) -> Value {
    let job_count = m.job_count;
    let mttr      = norm(m.avg_repair_min,   15.0, 480.0, true);
    let resp      = norm(m.avg_response_min,  5.0, 120.0, true);
    let ftfr      = norm(m.ftfr_pct,         20.0,  80.0, false);
    let vol       = norm(if avg_jobs > 0.0 { job_count / avg_jobs } else { 0.0 }, 0.3, 1.5, false);
    let vers      = norm(m.area_count as f64, 0.0,   3.0, false);
    let score     = ((mttr * 0.30 + resp * 0.20 + ftfr * 0.25 + vol * 0.15 + vers * 0.10) * 10.0).round() / 10.0;
    let grade     = if score >= 85.0 { "A" } else if score >= 70.0 { "B" } else if score >= 55.0 { "C" } else { "D" };

    json!({
        "technician":        m.technician,
        "supervisor":        null,
        "score":             score,
        "grade":             grade,
        "mttr_score":        mttr,
        "response_score":    resp,
        "ftfr_score":        ftfr,
        "volume_score":      vol,
        "versatility_score": vers,
        "job_count":         job_count as i64,
        "avg_response_min":  m.avg_response_min,
        "avg_repair_min":    m.avg_repair_min,
        "area_count":        m.area_count,
        "ftfr_pct":          m.ftfr_pct,
    })
}
