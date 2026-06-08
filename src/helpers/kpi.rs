use std::collections::HashMap;

use crate::helpers::where_builder::{DOWN_TYPES, LOST_TYPES, PM_TYPES};

/// Round ทศนิยม 2 ตำแหน่ง (port r2)
pub fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}


/// คำนวณจำนวนวันระหว่าง start–end (port nDays)
pub fn n_days(start: Option<&str>, end: Option<&str>) -> i64 {
    let (Some(s), Some(e)) = (start, end) else { return 30 };
    match (
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d"),
        chrono::NaiveDate::parse_from_str(e, "%Y-%m-%d"),
    ) {
        (Ok(sd), Ok(ed)) => std::cmp::max(1, (ed - sd).num_days() + 1),
        _ => 30,
    }
}

/// ชั่วโมงต่อ shift
fn shift_hours(shift: Option<&str>) -> f64 {
    match shift.map(|s| s.to_uppercase()).as_deref() {
        Some("DAY") | Some("NIGHT") => 12.0,
        _ => 24.0,
    }
}

pub struct KpiRow {
    pub job_type:  String,
    pub total_min: f64,
    pub wait_min:  f64,
}

pub struct KpiResult {
    pub utilization_pct: f64,
    pub downtime_pct:    f64,
    pub pm_pct:          f64,
    pub lost_time_pct:   f64,
}

/// port computeKpis()
pub fn compute_kpis(rows: &[KpiRow], mc_count: i64, n_days_val: i64, shift: Option<&str>) -> KpiResult {
    let h = shift_hours(shift);
    let avail = (mc_count as f64 * n_days_val as f64 * h * 60.0).max(1.0);
    let (mut dn, mut pm, mut ls, mut wt) = (0.0f64, 0.0, 0.0, 0.0);

    for r in rows {
        let tot = r.total_min;
        let w   = r.wait_min;
        if DOWN_TYPES.contains(&r.job_type.as_str())  { dn += tot; }
        else if PM_TYPES.contains(&r.job_type.as_str()) { pm += tot; }
        else if LOST_TYPES.contains(&r.job_type.as_str()) { ls += tot; }
        wt += w;
    }
    ls += wt;
    let (dp, pp, lp) = (r2(dn / avail * 100.0), r2(pm / avail * 100.0), r2(ls / avail * 100.0));
    KpiResult {
        utilization_pct: r2((100.0 - dp - pp - lp).max(0.0)),
        downtime_pct:    dp,
        pm_pct:          pp,
        lost_time_pct:   lp,
    }
}

pub struct AreaRow {
    pub area:      String,
    pub job_type:  String,
    pub total_min: f64,
    pub wait_min:  f64,
}

pub struct McCountRow {
    pub area:          String,
    pub machine_count: i64,
}

pub struct AreaUtilResult {
    pub area:            String,
    pub utilization_pct: f64,
    pub target_pct:      f64,
}

/// port areaUtil()
pub fn area_util(
    area_rows: &[AreaRow],
    mc_cnt_rows: &[McCountRow],
    n_days_val: i64,
    shift: Option<&str>,
) -> Vec<AreaUtilResult> {
    let h = shift_hours(shift);
    let mc_map: HashMap<&str, i64> = mc_cnt_rows.iter().map(|r| (r.area.as_str(), r.machine_count)).collect();

    let mut grouped: HashMap<&str, Vec<&AreaRow>> = HashMap::new();
    for r in area_rows {
        grouped.entry(r.area.as_str()).or_default().push(r);
    }

    let mut result: Vec<AreaUtilResult> = grouped.iter().map(|(area, rows)| {
        let mc = *mc_map.get(area).unwrap_or(&1);
        let avail = (mc.max(1) as f64) * n_days_val as f64 * h * 60.0;
        let (mut dn, mut pm, mut ls, mut wt) = (0.0f64, 0.0, 0.0, 0.0);
        for r in rows.iter() {
            let tot = r.total_min;
            let w   = r.wait_min;
            if DOWN_TYPES.contains(&r.job_type.as_str())  { dn += tot; }
            else if PM_TYPES.contains(&r.job_type.as_str()) { pm += tot; }
            else if LOST_TYPES.contains(&r.job_type.as_str()) { ls += tot; }
            wt += w;
        }
        ls += wt;
        AreaUtilResult {
            area: area.to_string(),
            utilization_pct: r2((100.0 - (dn + pm + ls) / avail * 100.0).max(0.0)),
            target_pct: 85.0,
        }
    }).collect();

    result.sort_by(|a, b| a.area.cmp(&b.area));
    result
}

pub struct MonthlyRow {
    pub ym:        String,
    pub job_type:  String,
    pub total_min: f64,
    pub wait_min:  f64,
}

pub struct MonthlyResult {
    pub month:       String,
    pub running_min: f64,
    pub down_min:    f64,
    pub pm_min:      f64,
    pub lost_min:    f64,
}

/// port monthlyTrend()
pub fn monthly_trend(rows: &[MonthlyRow], mc_count: i64, shift: Option<&str>) -> Vec<MonthlyResult> {
    let h = shift_hours(shift);
    let mut grouped: HashMap<&str, Vec<&MonthlyRow>> = HashMap::new();
    for r in rows {
        grouped.entry(r.ym.as_str()).or_default().push(r);
    }

    let mut result: Vec<MonthlyResult> = grouped.iter().map(|(ym, grp)| {
        let days = parse_month_days(ym);
        let avail = mc_count as f64 * days as f64 * h * 60.0;
        let (mut dn, mut pm, mut ls, mut wt) = (0.0f64, 0.0, 0.0, 0.0);
        for r in grp.iter() {
            let tot = r.total_min;
            let w   = r.wait_min;
            if DOWN_TYPES.contains(&r.job_type.as_str())  { dn += tot; }
            else if PM_TYPES.contains(&r.job_type.as_str()) { pm += tot; }
            else if LOST_TYPES.contains(&r.job_type.as_str()) { ls += tot; }
            wt += w;
        }
        ls += wt;
        MonthlyResult {
            month: ym.to_string(),
            running_min: (avail - dn - pm - ls).max(0.0),
            down_min: dn, pm_min: pm, lost_min: ls,
        }
    }).collect();

    result.sort_by(|a, b| a.month.cmp(&b.month));
    result
}

/// "2024-03" → จำนวนวันในเดือน
fn parse_month_days(ym: &str) -> i64 {
    let parts: Vec<&str> = ym.splitn(2, '-').collect();
    if parts.len() < 2 { return 30; }
    let (yr, mo): (i32, u32) = match (parts[0].parse(), parts[1].parse()) {
        (Ok(y), Ok(m)) => (y, m),
        _ => return 30,
    };
    // วันสุดท้ายของเดือน = วันที่ 1 ของเดือนถัดไป - 1 วัน
    let next_month = if mo == 12 { chrono::NaiveDate::from_ymd_opt(yr + 1, 1, 1) }
                     else { chrono::NaiveDate::from_ymd_opt(yr, mo + 1, 1) };
    match next_month {
        Some(d) => (d - chrono::NaiveDate::from_ymd_opt(yr, mo, 1).unwrap()).num_days(),
        None => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r2_rounds_correctly() {
        assert_eq!(r2(1.226), 1.23);   // 1.226 * 100 = 122.6 → 123 → 1.23
        assert_eq!(r2(85.333), 85.33); // 8533.3 → 8533 → 85.33
        assert_eq!(r2(85.336), 85.34); // 8533.6 → 8534 → 85.34
        assert_eq!(r2(0.0), 0.0);
        assert_eq!(r2(100.0), 100.0);
    }

    #[test]
    fn n_days_calculates_correctly() {
        assert_eq!(n_days(Some("2024-01-01"), Some("2024-01-31")), 31);
        assert_eq!(n_days(Some("2024-01-01"), Some("2024-01-01")), 1);
        assert_eq!(n_days(None, None), 30);
    }

    #[test]
    fn compute_kpis_all_running() {
        let kpi = compute_kpis(&[], 10, 1, None);
        assert_eq!(kpi.utilization_pct, 100.0);
        assert_eq!(kpi.downtime_pct, 0.0);
    }

    #[test]
    fn parse_month_days_correct() {
        assert_eq!(parse_month_days("2024-01"), 31);
        assert_eq!(parse_month_days("2024-02"), 29); // leap year
        assert_eq!(parse_month_days("2023-02"), 28);
    }
}
