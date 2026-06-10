// Oracle-side aggregations — port of utils/oracle_agg.py. Each takes filtered ISO/FS
// rows and returns the SAME row types the MSSQL repos build, so results merge by
// extending the vecs before the existing kpi.rs helpers run (which group + sum).

use std::collections::{HashMap, HashSet};

use crate::helpers::kpi::{AreaRow, KpiRow, McCountRow, MonthlyRow};
use super::model::OracleRow;

const DOWN: &str = "M/C DOWN";

/// group by job_type → total_min = Σ repair_min, wait_min = Σ wait_min
pub fn ora_kpi_totals(rows: &[OracleRow]) -> Vec<KpiRow> {
    let mut m: HashMap<String, (f64, f64)> = HashMap::new();
    for r in rows {
        let e = m.entry(r.job_type.clone()).or_insert((0.0, 0.0));
        e.0 += r.repair_min;
        e.1 += r.wait_min;
    }
    m.into_iter()
        .map(|(job_type, (total_min, wait_min))| KpiRow { job_type, total_min, wait_min })
        .collect()
}

/// group by (YYYY-MM of datex, job_type)
pub fn ora_monthly(rows: &[OracleRow]) -> Vec<MonthlyRow> {
    let mut m: HashMap<(String, String), (f64, f64)> = HashMap::new();
    for r in rows {
        let Some(dt) = r.datex else { continue };
        let ym = dt.format("%Y-%m").to_string();
        let e = m.entry((ym, r.job_type.clone())).or_insert((0.0, 0.0));
        e.0 += r.repair_min;
        e.1 += r.wait_min;
    }
    m.into_iter()
        .map(|((ym, job_type), (total_min, wait_min))| MonthlyRow { ym, job_type, total_min, wait_min })
        .collect()
}

/// group by (area, job_type)
pub fn ora_by_area(rows: &[OracleRow]) -> Vec<AreaRow> {
    let mut m: HashMap<(String, String), (f64, f64)> = HashMap::new();
    for r in rows {
        let e = m.entry((r.area.clone(), r.job_type.clone())).or_insert((0.0, 0.0));
        e.0 += r.repair_min;
        e.1 += r.wait_min;
    }
    m.into_iter()
        .map(|((area, job_type), (total_min, wait_min))| AreaRow { area, job_type, total_min, wait_min })
        .collect()
}

/// distinct machine_id per area
pub fn ora_machine_count(rows: &[OracleRow]) -> Vec<McCountRow> {
    let mut m: HashMap<String, HashSet<&str>> = HashMap::new();
    for r in rows {
        m.entry(r.area.clone()).or_default().insert(r.machine_id.as_str());
    }
    m.into_iter()
        .map(|(area, set)| McCountRow { area, machine_count: set.len() as i64 })
        .collect()
}

/// distinct machine_id (total)
pub fn ora_total_machine_count(rows: &[OracleRow]) -> i64 {
    rows.iter().map(|r| r.machine_id.as_str()).collect::<HashSet<_>>().len() as i64
}

/// Scatter row (M/C DOWN only): freq ≥ 2 machines.
pub struct ScatterRow {
    pub machine_id: String,
    pub area: String,
    pub freq: i64,
    pub avg_dur_min: f64,
    pub total_hours: f64,
}

/// M/C DOWN, group by (machine_id, area): freq, avg duration, total hours; keep freq ≥ 2.
pub fn ora_freq_vs_duration(rows: &[OracleRow]) -> Vec<ScatterRow> {
    let mut m: HashMap<(String, String), (i64, f64)> = HashMap::new(); // (count, sum_repair)
    for r in rows.iter().filter(|r| r.job_type == DOWN) {
        let e = m.entry((r.machine_id.clone(), r.area.clone())).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += r.repair_min;
    }
    m.into_iter()
        .filter(|(_, (c, _))| *c >= 2)
        .map(|((machine_id, area), (c, sum))| ScatterRow {
            machine_id,
            area,
            freq: c,
            avg_dur_min: ((sum / c as f64) * 10.0).round() / 10.0,
            total_hours: ((sum / 60.0) * 10.0).round() / 10.0,
        })
        .collect()
}

// ── Downtime detail ─────────────────────────────────────────────────────────

fn in_jt(r: &OracleRow, jt: &[&str]) -> bool {
    jt.iter().any(|j| *j == r.job_type)
}
/// reason text = symptom (CRITERIA) when the request uses des_job, else cause.
fn reason_of(r: &OracleRow, use_symptom: bool) -> &str {
    if use_symptom { &r.symptom } else { &r.cause }
}

/// reason → (count, Σrepair_min). Caller merges with SQL then computes hours/avg.
pub struct DtReason { pub reason: String, pub cnt: i64, pub sum_min: f64 }
pub fn ora_dt_reason(rows: &[OracleRow], jt: &[&str], use_symptom: bool) -> Vec<DtReason> {
    let mut m: HashMap<String, (i64, f64)> = HashMap::new();
    for r in rows.iter().filter(|r| in_jt(r, jt)) {
        let reason = reason_of(r, use_symptom);
        if reason.is_empty() { continue; }
        let e = m.entry(reason.to_string()).or_insert((0, 0.0));
        e.0 += 1;
        e.1 += r.repair_min;
    }
    m.into_iter().map(|(reason, (cnt, sum_min))| DtReason { reason, cnt, sum_min }).collect()
}

/// (machine_id, area, reason) → hours.
pub struct DtMachine { pub machine_id: String, pub area: String, pub reason: String, pub hours: f64 }
pub fn ora_dt_machine(rows: &[OracleRow], jt: &[&str], use_symptom: bool) -> Vec<DtMachine> {
    let mut m: HashMap<(String, String, String), f64> = HashMap::new();
    for r in rows.iter().filter(|r| in_jt(r, jt)) {
        let reason = reason_of(r, use_symptom);
        if reason.is_empty() { continue; }
        *m.entry((r.machine_id.clone(), r.area.clone(), reason.to_string())).or_insert(0.0) += r.repair_min / 60.0;
    }
    m.into_iter().map(|((machine_id, area, reason), hours)| DtMachine { machine_id, area, reason, hours }).collect()
}

/// (day, shift_name) → events, Σrepair_min, Σwait_min. day = datex date; shift_name from the
/// Oracle SHIFT column (D/N), defaulting to Day for blank/unknown — matches the Python port
/// (oracle_agg.ora_dt_daily_shift). NB: do NOT derive shift from datex hour — S_DATE carries
/// no usable time-of-day, so every row would collapse to Night.
pub struct DtDaily { pub day: String, pub shift_name: String, pub events: i64, pub repair_min: f64, pub wait_min: f64 }
pub fn ora_dt_daily(rows: &[OracleRow], jt: &[&str]) -> Vec<DtDaily> {
    let mut m: HashMap<(String, String), (i64, f64, f64)> = HashMap::new();
    for r in rows.iter().filter(|r| in_jt(r, jt)) {
        let Some(dt) = r.datex else { continue };
        let day = dt.date().format("%Y-%m-%d").to_string();
        let shift_name = if r.shift_code == "N" { "Night" } else { "Day" };
        let e = m.entry((day, shift_name.to_string())).or_insert((0, 0.0, 0.0));
        e.0 += 1;
        e.1 += r.repair_min;
        e.2 += r.wait_min;
    }
    m.into_iter().map(|((day, shift_name), (events, repair_min, wait_min))| DtDaily {
        day, shift_name, events, repair_min, wait_min,
    }).collect()
}

// ── Downtime events (raw rows for the Event Detail table) ─────────────────────

pub struct DtEvent {
    pub event_time: String, pub machine_id: String, pub area: String, pub job_type: String,
    pub symptom: String, pub cause: String, pub tech: String,
    pub wait_min: i64, pub repair_min: i64,
    pub die_mask: String, pub lot_no: String, pub package_type: String,
}

/// Raw ISO/FS event rows matching the MSSQL events() shape. Applies the same optional
/// filters the SQL query applies (job_types, machine, symptom, cause, tech); date/area/shift
/// are already filtered by OracleCache::filter_historical. tech = BADGE_NO — Oracle's
/// historical view has no performer-name column, so the badge stands in for it.
pub fn ora_dt_events(
    rows: &[OracleRow], jt: &[&str],
    machine: Option<&str>, symptom: Option<&str>, cause: Option<&str>, tech: Option<&str>,
) -> Vec<DtEvent> {
    rows.iter()
        .filter(|r| in_jt(r, jt))
        .filter(|r| machine.is_none_or(|m| r.machine_id.trim() == m.trim()))
        .filter(|r| symptom.is_none_or(|s| r.symptom == s))
        .filter(|r| cause.is_none_or(|c| r.cause == c))
        .filter(|r| tech.is_none_or(|t| r.badge == t))
        .map(|r| DtEvent {
            event_time: r.datex.map(|dt| dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap_or_default(),
            machine_id: r.machine_id.clone(),
            area: r.area.clone(),
            job_type: r.job_type.clone(),
            symptom: r.symptom.clone(),
            cause: r.cause.clone(),
            tech: r.badge.clone(),
            wait_min: r.wait_min.round() as i64,
            repair_min: r.repair_min.round() as i64,
            die_mask: r.die_mask.clone(),
            lot_no: r.lot_no.clone(),
            package_type: r.package_type.clone(),
        })
        .collect()
}

// ── Tech metrics (incl. FTFR) ─────────────────────────────────────────────────

pub struct TechMetric {
    pub technician: String,
    pub job_count: f64,
    pub avg_response_min: f64,
    pub avg_repair_min: f64,
    pub area_count: i64,
    pub ftfr_pct: f64,
}

/// Per-technician metrics from Oracle rows, including FTFR (first-time-fix rate):
/// for M/C DOWN sorted by (badge, machine, datex), a fix is "first-time" if there's
/// no next job on that machine within 7 days.
pub fn ora_tech_metrics(rows: &[OracleRow]) -> Vec<TechMetric> {
    use std::collections::HashSet;
    // base metrics
    struct Acc { jobs: i64, resp: f64, repair: f64, areas: HashSet<String> }
    let mut base: HashMap<String, Acc> = HashMap::new();
    for r in rows.iter().filter(|r| !r.badge.is_empty()) {
        let a = base.entry(r.badge.clone()).or_insert(Acc { jobs: 0, resp: 0.0, repair: 0.0, areas: HashSet::new() });
        a.jobs += 1;
        a.resp += r.wait_min;
        a.repair += r.repair_min;
        a.areas.insert(r.area.clone());
    }

    // FTFR: M/C DOWN, grouped by (badge, machine), ordered by datex
    let mut groups: HashMap<(String, String), Vec<&OracleRow>> = HashMap::new();
    for r in rows.iter().filter(|r| !r.badge.is_empty() && r.job_type == DOWN) {
        groups.entry((r.badge.clone(), r.machine_id.clone())).or_default().push(r);
    }
    let mut ftfr: HashMap<String, (i64, i64)> = HashMap::new(); // badge -> (mc_total, first_fixes)
    for ((badge, _mc), mut grp) in groups {
        grp.sort_by_key(|r| r.datex);
        for i in 0..grp.len() {
            let is_ftf = match (grp[i].date_close, grp.get(i + 1).and_then(|n| n.datex)) {
                (Some(close), Some(next)) => (next - close).num_days() > 7,
                _ => true, // no next job → first-time fix
            };
            let e = ftfr.entry(badge.clone()).or_insert((0, 0));
            e.0 += 1;
            if is_ftf { e.1 += 1; }
        }
    }

    base.into_iter().map(|(technician, a)| {
        let jc = a.jobs as f64;
        let ftfr_pct = match ftfr.get(&technician) {
            Some(&(total, fixes)) if total > 0 => ((fixes as f64 / total as f64 * 100.0) * 10.0).round() / 10.0,
            _ => 100.0,
        };
        TechMetric {
            technician,
            job_count: jc,
            avg_response_min: ((a.resp / jc) * 10.0).round() / 10.0,
            avg_repair_min: ((a.repair / jc) * 10.0).round() / 10.0,
            area_count: a.areas.len() as i64,
            ftfr_pct,
        }
    }).collect()
}
