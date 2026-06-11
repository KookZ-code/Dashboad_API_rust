// DA-UPH module — PostgreSQL `uph` database (Die Attach hourly scan output).
//
// The Die Attach analog of wb_uph_repo, but a different data store and model:
//   - WB reads SQLite central.db where `bonded_unit` is a CUMULATIVE counter that
//     needs reset-aware delta replay. DA's `output_record.qty_good` is INCREMENTAL
//     per record, so production is a plain SUM(qty_good) — none of the
//     scan_delta / reset_aware_total / baseline machinery from wb_uph is needed.
//   - DA timestamps live in `output_record.ts` as TEXT "YYYY-MM-DD HH:MM:SS"
//     (confirmed via v_hourly_output's `substr(ts,1,13)` bucket), so the same
//     lexicographic window comparison as WB works; the hour is `substr(ts,12,2)`.
//   - machine/operator/package are integer FKs resolved by JOIN:
//       machine.code (text id) · operator.badge · wafer_lot.package/.mpc
//
// sqlx Postgres is async, so these are plain `async fn` — NO spawn_blocking and
// NO file-mirror cache (those exist only for WB's SQLite-over-SMB share).
//
// Every function returns RAW numbers as serde_json::Value; the SvelteKit BFF
// layers the A01 plan/target on top, exactly as it does for wb-uph.

use anyhow::Result;
use chrono::{Local, NaiveDate, NaiveDateTime, Timelike};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

// Package key shared by every query: "PACKAGE(MPC)" when an MPC code exists, else
// the bare package. Matches the frontend's mpcCode() regex \(([A-Za-z0-9]+)\).
// `wl` is the wafer_lot alias in each query.
const PKG_KEY: &str =
    "CASE WHEN wl.mpc IS NOT NULL AND wl.mpc <> '' \
          THEN wl.package || '(' || wl.mpc || ')' \
          ELSE COALESCE(wl.package, '') END";

// ─── Shift window (port of wb_uph_repo / shift.ts) ──────────────────────────────

pub struct ShiftWin {
    pub start: String, // "YYYY-MM-DD HH:MM:SS"
    pub end: String,
    pub hours: Vec<u32>,
}

fn prev_day(date: &str) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.pred_opt())
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| date.to_string())
}

fn shift_window(date: &str, shift: &str) -> ShiftWin {
    if shift == "D" {
        ShiftWin {
            start: format!("{date} 07:00:00"),
            end: format!("{date} 18:59:59"),
            hours: (7..=18).collect(),
        }
    } else {
        let prev = prev_day(date);
        let mut hours: Vec<u32> = (19..=23).collect();
        hours.extend(0..=6);
        ShiftWin {
            start: format!("{prev} 19:00:00"),
            end: format!("{date} 06:59:59"),
            hours,
        }
    }
}

/// Fill (date, shift) defaults from "now" — mirrors handler-utils.resolveShift.
pub fn resolve_shift(date: Option<&str>, shift: Option<&str>) -> (String, String) {
    let (cur_date, cur_shift) = current_shift();
    let date = match date {
        Some(d) if is_valid_date(d) => d.to_string(),
        _ => cur_date,
    };
    let shift = match shift {
        Some("D") => "D".to_string(),
        Some("N") => "N".to_string(),
        _ => cur_shift,
    };
    (date, shift)
}

fn is_valid_date(d: &str) -> bool {
    d.len() == 10 && NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()
}

fn current_shift() -> (String, String) {
    let now = Local::now().naive_local();
    let h = now.time().hour();
    if (7..19).contains(&h) {
        (now.date().format("%Y-%m-%d").to_string(), "D".into())
    } else if h >= 19 {
        let tomorrow = now.date().succ_opt().unwrap_or(now.date());
        (tomorrow.format("%Y-%m-%d").to_string(), "N".into())
    } else {
        (now.date().format("%Y-%m-%d").to_string(), "N".into())
    }
}

/// End-of-hour timestamp for a slot, clamped to shift end (port of slotEndForHour).
fn slot_end_for_hour(w: &ShiftWin, hour: u32) -> String {
    let date = if hour <= 6 { &w.end[..10] } else { &w.start[..10] };
    let slot = format!("{date} {hour:02}:59:59");
    if slot < w.end { slot } else { w.end.clone() }
}

/// Parse "YYYY-MM-DD HH:MM:SS" to epoch seconds (only relative diffs are used).
fn parse_ts_secs(ts: &str) -> i64 {
    NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0)
}

// ─── 1. Summary ─────────────────────────────────────────────────────────────────

pub async fn query_summary(pool: &PgPool, date: &str, shift: &str, pkgs: &[String]) -> Result<Value> {
    let w = shift_window(date, shift);
    let has_filter = !pkgs.is_empty();
    let pkg_clause = if has_filter { format!("AND ({PKG_KEY}) = ANY($3)") } else { String::new() };
    let sql = format!(
        "SELECT COALESCE(SUM(o.qty_good), 0)::int8 AS total_bonded,
                COUNT(DISTINCT o.machine_id)        AS active_machines,
                COUNT(DISTINCT o.operator_id)       AS active_operators
         FROM output_record o
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.ts >= $1 AND o.ts <= $2 {pkg_clause}"
    );
    let mut q = sqlx::query(&sql).bind(&w.start).bind(&w.end);
    if has_filter { q = q.bind(pkgs); }
    let row = q.fetch_one(pool).await?;
    Ok(json!({
        "total_bonded":     row.get::<i64, _>("total_bonded"),
        "active_machines":  row.get::<i64, _>("active_machines"),
        "active_operators": row.get::<i64, _>("active_operators"),
    }))
}

// ─── 2. Hourly (cumulative-to-slot, like wb_uph) ─────────────────────────────────
//
// MainChart compares each stacked bar's total against target_cumulative directly,
// so per-package values must be CUMULATIVE through the slot. We bucket per hour in
// SQL, then build a running sum across the shift's hour slots in Rust.

pub async fn query_hourly(pool: &PgPool, date: &str, shift: &str, pkgs: &[String]) -> Result<Value> {
    let w = shift_window(date, shift);
    let has_filter = !pkgs.is_empty();
    let pkg_clause = if has_filter { format!("AND ({PKG_KEY}) = ANY($3)") } else { String::new() };
    let sql = format!(
        "SELECT ({PKG_KEY}) AS pkg_key,
                substr(o.ts, 12, 2)::int AS hr,
                COALESCE(SUM(o.qty_good), 0)::int8 AS bonded
         FROM output_record o
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.ts >= $1 AND o.ts <= $2 {pkg_clause}
         GROUP BY pkg_key, hr"
    );
    let mut q = sqlx::query(&sql).bind(&w.start).bind(&w.end);
    if has_filter { q = q.bind(pkgs); }
    let rows = q.fetch_all(pool).await?;

    let n = w.hours.len();
    // pkg -> per-hour bucket (not yet cumulative)
    let mut buckets: HashMap<String, Vec<i64>> = HashMap::new();
    for r in &rows {
        let pkg: String = r.get("pkg_key");
        let hr: i32 = r.get("hr");
        let bonded: i64 = r.get("bonded");
        if let Some(slot) = w.hours.iter().position(|&h| h as i32 == hr) {
            buckets.entry(pkg).or_insert_with(|| vec![0; n])[slot] += bonded;
        }
    }
    // running sum → cumulative-to-slot
    let mut pkg_map: HashMap<String, Vec<i64>> = HashMap::new();
    for (pkg, per_hour) in buckets {
        let mut acc = 0i64;
        let cum: Vec<i64> = per_hour.iter().map(|&v| { acc += v; acc }).collect();
        pkg_map.insert(pkg, cum);
    }
    Ok(json!({ "packages": pkg_map }))
}

// ─── 3. Packages (summed through the slot's hour) ────────────────────────────────

#[derive(Serialize)]
struct PackageRaw {
    package: String,
    bonded: i64,
}

pub async fn query_packages(
    pool: &PgPool, date: &str, shift: &str, hour: Option<u32>, pkgs: &[String],
) -> Result<Value> {
    let w = shift_window(date, shift);
    let hour = hour.unwrap_or_else(|| *w.hours.last().unwrap_or(&18));
    let slot_end = slot_end_for_hour(&w, hour);
    let has_filter = !pkgs.is_empty();
    let pkg_clause = if has_filter { format!("AND ({PKG_KEY}) = ANY($3)") } else { String::new() };
    let sql = format!(
        // GROUP BY the full expression, not the `package` alias — wafer_lot has a
        // real column named `package`, and in GROUP BY an input column name wins
        // over an output alias, which would group by the wrong thing.
        "SELECT ({PKG_KEY}) AS package, COALESCE(SUM(o.qty_good), 0)::int8 AS bonded
         FROM output_record o
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.ts >= $1 AND o.ts <= $2 {pkg_clause}
         GROUP BY ({PKG_KEY})
         ORDER BY bonded DESC"
    );
    let mut q = sqlx::query(&sql).bind(&w.start).bind(&slot_end);
    if has_filter { q = q.bind(pkgs); }
    let rows = q.fetch_all(pool).await?;
    let out: Vec<PackageRaw> = rows.iter()
        .map(|r| PackageRaw { package: r.get("package"), bonded: r.get("bonded") })
        .collect();
    Ok(serde_json::to_value(out)?)
}

// ─── 4. Machines (one package) ───────────────────────────────────────────────────

#[derive(Serialize)]
struct MachineRaw {
    machine_id: String,
    badge_no: String,
    uph: f64,
    bonded_unit: i64,
    last_scan_ts: Option<String>,
    pkg_mpc: String,
    target_uph: i64,
}

pub async fn query_machines(
    pool: &PgPool, date: &str, shift: &str, hour: Option<u32>, package: &str,
) -> Result<Value> {
    let w = shift_window(date, shift);
    let hour = hour.unwrap_or_else(|| *w.hours.last().unwrap_or(&18));
    let slot_end = slot_end_for_hour(&w, hour);

    // Base key (no parens) catches MPC variants but not space-qualified variants —
    // mirror wb_uph_repo's branch.
    let pkg_match = if package.contains('(') {
        format!("({PKG_KEY}) = $3")
    } else {
        format!("(wl.package = $3 OR ({PKG_KEY}) LIKE $3 || '(%')")
    };

    let sql = format!(
        "SELECT m.code AS machine_id,
                COALESCE((array_agg(op.badge ORDER BY o.ts DESC))[1], '') AS badge_no,
                COALESCE(SUM(o.qty_good), 0)::int8 AS bonded_unit,
                MAX(o.ts) AS last_scan_ts,
                COALESCE(MAX(m.target_uph), 0)::int8 AS target_uph,
                COALESCE((array_agg(({PKG_KEY}) ORDER BY o.ts DESC))[1], '') AS pkg_mpc
         FROM output_record o
         JOIN machine m        ON m.id = o.machine_id
         LEFT JOIN operator op ON op.id = o.operator_id
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.ts >= $1 AND o.ts <= $2 AND {pkg_match}
         GROUP BY m.code
         ORDER BY bonded_unit DESC"
    );
    let rows = sqlx::query(&sql)
        .bind(&w.start).bind(&slot_end).bind(package)
        .fetch_all(pool).await?;

    let start_secs = parse_ts_secs(&w.start);
    let out: Vec<MachineRaw> = rows.iter().map(|r| {
        let bonded_unit: i64 = r.get("bonded_unit");
        let last_scan_ts: Option<String> = r.get("last_scan_ts");
        // uph = average rate over elapsed shift time (DA has no per-record uph).
        // The frontend uses target_uph (native) for its vs-target math; this is the
        // displayed actual rate only. Guard the denominator near shift start.
        let uph = match &last_scan_ts {
            Some(ts) => {
                let elapsed_h = (parse_ts_secs(ts) - start_secs) as f64 / 3600.0;
                if elapsed_h > 0.0 { bonded_unit as f64 / elapsed_h } else { 0.0 }
            }
            None => 0.0,
        };
        MachineRaw {
            machine_id: r.get("machine_id"),
            badge_no: r.get("badge_no"),
            uph,
            bonded_unit,
            last_scan_ts,
            pkg_mpc: r.get("pkg_mpc"),
            target_uph: r.get("target_uph"),
        }
    }).collect();
    Ok(serde_json::to_value(out)?)
}

// ─── 5. Records ──────────────────────────────────────────────────────────────────
//
// DA is incremental, so delta_bonded == bonded_unit == qty_good per record (no LAG,
// no reset replay). uph has no per-record source → 0.0 (RecordsTable shows package).

#[derive(Serialize)]
struct RawRecord {
    created_at: String,
    lot_id: String,
    package_mpc: String,
    uph: f64,
    bonded_unit: i64,
    delta_bonded: i64,
    badge_no: String,
}

pub async fn query_records(
    pool: &PgPool, date: &str, shift: &str, machine_id: &str, package: &str,
) -> Result<Value> {
    let w = shift_window(date, shift);
    let pkg_match = if package.contains('(') {
        format!("({PKG_KEY}) = $3")
    } else {
        format!("(wl.package = $3 OR ({PKG_KEY}) LIKE $3 || '(%')")
    };

    let base_select = format!(
        "SELECT o.ts AS created_at,
                COALESCE(o.mtai_lot_id, o.wafer_lot_id, '') AS lot_id,
                ({PKG_KEY}) AS package_mpc,
                o.qty_good AS qty_good,
                COALESCE(op.badge, '') AS badge_no
         FROM output_record o
         JOIN machine m        ON m.id = o.machine_id
         LEFT JOIN operator op ON op.id = o.operator_id
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE m.code = $1 AND {pkg_match}"
    );

    let map_row = |r: &sqlx::postgres::PgRow, with_delta: bool| {
        let qty: i32 = r.get("qty_good");
        RawRecord {
            created_at: r.get("created_at"),
            lot_id: r.get("lot_id"),
            package_mpc: r.get("package_mpc"),
            uph: 0.0,
            bonded_unit: qty as i64,
            delta_bonded: if with_delta { qty as i64 } else { 0 },
            badge_no: r.get("badge_no"),
        }
    };

    // current shift, oldest → newest
    let current_sql = format!("{base_select} AND o.ts >= $4 AND o.ts <= $5 ORDER BY o.ts ASC");
    let current: Vec<RawRecord> = sqlx::query(&current_sql)
        .bind(machine_id).bind(package).bind(package).bind(&w.start).bind(&w.end)
        .fetch_all(pool).await?
        .iter().map(|r| map_row(r, true)).collect();

    // previous-shift tail — last 5 before shift start, oldest first, delta=0
    let prev_sql = format!("{base_select} AND o.ts < $4 ORDER BY o.ts DESC LIMIT 5");
    let mut prev_tail: Vec<RawRecord> = sqlx::query(&prev_sql)
        .bind(machine_id).bind(package).bind(package).bind(&w.start)
        .fetch_all(pool).await?
        .iter().map(|r| map_row(r, false)).collect();
    prev_tail.reverse();

    Ok(json!({ "current": current, "prev_tail": prev_tail }))
}

// ─── 6. Monitor ──────────────────────────────────────────────────────────────────

const THRESHOLD_MIN: i64 = 120;

#[derive(Serialize)]
struct MonitorRow {
    machine_id: String,
    package: String,
    last_scan_ts: Option<String>,
    since_min: Option<i64>,
    status: String,
}

pub async fn query_monitor(pool: &PgPool, date: &str, shift: &str) -> Result<Value> {
    let w = shift_window(date, shift);
    let now = Local::now().naive_local();
    let now_min = parse_ts_secs(&now.format("%Y-%m-%d %H:%M:%S").to_string()) / 60;
    let as_of = now.format("%H:%M").to_string();

    // machines that scanned this shift (latest package + last scan per machine)
    let active_sql = format!(
        "SELECT m.code AS machine_id, MAX(o.ts) AS last_scan_ts,
                COALESCE((array_agg(({PKG_KEY}) ORDER BY o.ts DESC))[1], '') AS package
         FROM output_record o
         JOIN machine m        ON m.id = o.machine_id
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.ts >= $1 AND o.ts <= $2
         GROUP BY m.code"
    );
    let active_rows = sqlx::query(&active_sql).bind(&w.start).bind(&w.end).fetch_all(pool).await?;

    let mut rows: Vec<MonitorRow> = Vec::new();
    let mut active_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in &active_rows {
        let machine_id: String = r.get("machine_id");
        let last_scan_ts: String = r.get("last_scan_ts");
        active_ids.insert(machine_id.clone());
        let since_min = now_min - parse_ts_secs(&last_scan_ts) / 60;
        let status = if since_min <= THRESHOLD_MIN { "active" } else { "stale" };
        rows.push(MonitorRow {
            machine_id,
            package: r.get("package"),
            last_scan_ts: Some(last_scan_ts),
            since_min: Some(since_min),
            status: status.to_string(),
        });
    }

    // monitored fleet = active machines (machine.active = 1) that did NOT scan this shift → no_data
    let fleet_rows = sqlx::query("SELECT code FROM machine WHERE active = 1")
        .fetch_all(pool).await?;
    for r in &fleet_rows {
        let code: String = r.get("code");
        if active_ids.contains(&code) { continue; }
        rows.push(MonitorRow {
            machine_id: code,
            package: String::new(),
            last_scan_ts: None,
            since_min: None,
            status: "no_data".to_string(),
        });
    }

    // Sort: no_data → stale → active; within group most-stale first.
    let order = |s: &str| match s { "no_data" => 0, "stale" => 1, _ => 2 };
    rows.sort_by(|a, b| {
        let (oa, ob) = (order(&a.status), order(&b.status));
        if oa != ob { oa.cmp(&ob) }
        else { b.since_min.unwrap_or(9999).cmp(&a.since_min.unwrap_or(9999)) }
    });

    Ok(json!({ "rows": rows, "as_of": as_of, "threshold_min": THRESHOLD_MIN }))
}
