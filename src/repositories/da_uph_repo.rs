// DA-UPH module — PostgreSQL `uph` database (Die Attach hourly scan output).
//
// Unlike WB (SQLite central.db, cumulative bonded_unit, reset-aware delta),
// DA stores qty_good as an INCREMENTAL value per record → plain SUM(qty_good).
//
// TIME COLUMN: use `created_at` (timestamptz, server-local +07:00) NOT `ts`
// (which is UTC ISO text "2026-06-11T08:36:21Z"). All shift-window boundaries
// are passed as "+07"-suffixed strings so Postgres resolves the timezone correctly.
// Hourly buckets use EXTRACT(HOUR ... AT TIME ZONE 'Asia/Bangkok').
//
// sqlx Postgres is async — plain `async fn`, no spawn_blocking, no file cache.
// Every function returns raw serde_json::Value; SvelteKit BFF layers the plan/target.

use anyhow::Result;
use chrono::{Local, NaiveDate, Timelike};
use serde::Serialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
use std::collections::HashMap;

// "PACKAGE(CODE)" where CODE = 3-char variant slice of mpc (mirrors WB's
// SUBSTR(mpc,7,3)), else bare package. NOTE: DA's wafer_lot.mpc is the device
// part number, not A01's leadframe code, so this code does NOT match A01's
// parenthetical codes — A01 plan/WIP/DOI still attach via the frontend's normPkg
// name path (it strips the parens). Suffix kept only to mirror WB display.
const PKG_KEY: &str =
    "CASE WHEN wl.mpc IS NOT NULL AND LENGTH(wl.mpc) >= 9 \
          THEN wl.package || '(' || SUBSTR(wl.mpc, 7, 3) || ')' \
          ELSE COALESCE(wl.package, '') END";

// ─── Shift window ────────────────────────────────────────────────────────────
//
// Timestamps include "+07" so Postgres parses them as Asia/Bangkok timestamptz.
// String comparison in slot_end_for_hour still works because the format is uniform.

pub struct ShiftWin {
    pub start: String, // "YYYY-MM-DD HH:MM:SS+07"
    pub end:   String,
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
            start: format!("{date} 07:00:00+07"),
            end:   format!("{date} 18:59:59+07"),
            hours: (7..=18).collect(),
        }
    } else {
        let prev = prev_day(date);
        let mut hours: Vec<u32> = (19..=23).collect();
        hours.extend(0..=6);
        ShiftWin {
            start: format!("{prev} 19:00:00+07"),
            end:   format!("{date} 06:59:59+07"),
            hours,
        }
    }
}

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

/// End-of-hour boundary string for a given slot, clamped to shift end.
/// Both values carry "+07" suffix so lexicographic comparison is valid.
fn slot_end_for_hour(w: &ShiftWin, hour: u32) -> String {
    let date = if hour <= 6 { &w.end[..10] } else { &w.start[..10] };
    let slot = format!("{date} {hour:02}:59:59+07");
    if slot < w.end { slot } else { w.end.clone() }
}

// ─── 1. Summary ──────────────────────────────────────────────────────────────

pub async fn query_summary(pool: &PgPool, date: &str, shift: &str, pkgs: &[String]) -> Result<Value> {
    let w = shift_window(date, shift);
    let has_filter = !pkgs.is_empty();
    let pkg_clause = if has_filter { format!("AND ({PKG_KEY}) = ANY($3)") } else { String::new() };
    let sql = format!(
        "SELECT COALESCE(SUM(o.qty_good), 0)::int8    AS total_bonded,
                COUNT(DISTINCT o.machine_id)::int8    AS active_machines,
                COUNT(DISTINCT o.operator_id)::int8   AS active_operators
         FROM output_record o
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.created_at >= $1::timestamptz AND o.created_at <= $2::timestamptz {pkg_clause}"
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

// ─── 2. Hourly ───────────────────────────────────────────────────────────────
//
// Bucket by local hour via AT TIME ZONE, then build running sum in Rust.

pub async fn query_hourly(pool: &PgPool, date: &str, shift: &str, pkgs: &[String]) -> Result<Value> {
    let w = shift_window(date, shift);
    let has_filter = !pkgs.is_empty();
    let pkg_clause = if has_filter { format!("AND ({PKG_KEY}) = ANY($3)") } else { String::new() };
    let sql = format!(
        "SELECT ({PKG_KEY}) AS pkg_key,
                EXTRACT(HOUR FROM o.created_at AT TIME ZONE 'Asia/Bangkok')::int AS hr,
                COALESCE(SUM(o.qty_good), 0)::int8 AS bonded
         FROM output_record o
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.created_at >= $1::timestamptz AND o.created_at <= $2::timestamptz {pkg_clause}
         GROUP BY pkg_key, hr"
    );
    let mut q = sqlx::query(&sql).bind(&w.start).bind(&w.end);
    if has_filter { q = q.bind(pkgs); }
    let rows = q.fetch_all(pool).await?;

    let n = w.hours.len();
    let mut buckets: HashMap<String, Vec<i64>> = HashMap::new();
    for r in &rows {
        let pkg: String = r.get("pkg_key");
        let hr: i32     = r.get("hr");
        let bonded: i64 = r.get("bonded");
        if let Some(slot) = w.hours.iter().position(|&h| h as i32 == hr) {
            buckets.entry(pkg).or_insert_with(|| vec![0; n])[slot] += bonded;
        }
    }
    // incremental → cumulative running sum
    let pkg_map: HashMap<String, Vec<i64>> = buckets.into_iter().map(|(pkg, per_hour)| {
        let mut acc = 0i64;
        let cum: Vec<i64> = per_hour.iter().map(|&v| { acc += v; acc }).collect();
        (pkg, cum)
    }).collect();

    Ok(json!({ "packages": pkg_map }))
}

// ─── 3. Packages ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct PackageRaw { package: String, bonded: i64 }

pub async fn query_packages(
    pool: &PgPool, date: &str, shift: &str, hour: Option<u32>, pkgs: &[String],
) -> Result<Value> {
    let w = shift_window(date, shift);
    let hour     = hour.unwrap_or_else(|| *w.hours.last().unwrap_or(&18));
    let slot_end = slot_end_for_hour(&w, hour);
    let has_filter = !pkgs.is_empty();
    let pkg_clause = if has_filter { format!("AND ({PKG_KEY}) = ANY($3)") } else { String::new() };
    let sql = format!(
        "SELECT ({PKG_KEY}) AS package, COALESCE(SUM(o.qty_good), 0)::int8 AS bonded
         FROM output_record o
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.created_at >= $1::timestamptz AND o.created_at <= $2::timestamptz {pkg_clause}
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

// ─── 4. Machines ─────────────────────────────────────────────────────────────
//
// UPH = bonded / elapsed_h computed in SQL to avoid Rust timestamp parsing.
// elapsed_h = seconds from shift start to last scan / 3600; guarded against 0.

#[derive(Serialize)]
struct MachineRaw {
    machine_id:   String,
    badge_no:     String,
    uph:          f64,
    bonded_unit:  i64,
    last_scan_ts: Option<String>,
    pkg_mpc:      String,
    target_uph:   i64,
}

pub async fn query_machines(
    pool: &PgPool, date: &str, shift: &str, hour: Option<u32>, package: &str,
) -> Result<Value> {
    let w = shift_window(date, shift);
    let hour     = hour.unwrap_or_else(|| *w.hours.last().unwrap_or(&18));
    let slot_end = slot_end_for_hour(&w, hour);

    let pkg_match = if package.contains('(') {
        format!("({PKG_KEY}) = $3")
    } else {
        format!("(wl.package = $3 OR ({PKG_KEY}) LIKE $3 || '(%')")
    };

    let sql = format!(
        "SELECT m.code AS machine_id,
                COALESCE((array_agg(op.badge ORDER BY o.created_at DESC))[1], '') AS badge_no,
                COALESCE(SUM(o.qty_good), 0)::int8 AS bonded_unit,
                TO_CHAR(
                    MAX(o.created_at) AT TIME ZONE 'Asia/Bangkok',
                    'YYYY-MM-DD HH24:MI:SS'
                ) AS last_scan_ts,
                COALESCE(MAX(m.target_uph), 0)::int8 AS target_uph,
                COALESCE((array_agg(({PKG_KEY}) ORDER BY o.created_at DESC))[1], '') AS pkg_mpc,
                CASE WHEN EXTRACT(EPOCH FROM (MAX(o.created_at) - $1::timestamptz)) > 0
                     THEN COALESCE(SUM(o.qty_good), 0)::float8 /
                          (EXTRACT(EPOCH FROM (MAX(o.created_at) - $1::timestamptz)) / 3600.0)
                     ELSE 0.0
                END AS uph
         FROM output_record o
         JOIN machine m        ON m.id = o.machine_id
         LEFT JOIN operator op ON op.id = o.operator_id
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.created_at >= $1::timestamptz AND o.created_at <= $2::timestamptz AND {pkg_match}
         GROUP BY m.code
         ORDER BY bonded_unit DESC"
    );
    let rows = sqlx::query(&sql)
        .bind(&w.start).bind(&slot_end).bind(package)
        .fetch_all(pool).await?;

    let out: Vec<MachineRaw> = rows.iter().map(|r| MachineRaw {
        machine_id:   r.get("machine_id"),
        badge_no:     r.get("badge_no"),
        uph:          r.get("uph"),
        bonded_unit:  r.get("bonded_unit"),
        last_scan_ts: r.get("last_scan_ts"),
        pkg_mpc:      r.get("pkg_mpc"),
        target_uph:   r.get("target_uph"),
    }).collect();
    Ok(serde_json::to_value(out)?)
}

// ─── 5. Records ──────────────────────────────────────────────────────────────
//
// DA is incremental so delta_bonded == qty_good per record (no LAG/reset needed).
// created_at is formatted to local time string for display.

#[derive(Serialize)]
struct RawRecord {
    created_at:   String,
    lot_id:       String,
    package_mpc:  String,
    uph:          f64,
    bonded_unit:  i64,
    delta_bonded: i64,
    badge_no:     String,
}

pub async fn query_records(
    pool: &PgPool, date: &str, shift: &str, machine_id: &str, package: &str,
) -> Result<Value> {
    let w = shift_window(date, shift);
    // $1=machine_id  $2=package  $3=shift_start  ($4=shift_end for current only)
    let pkg_match = if package.contains('(') {
        format!("({PKG_KEY}) = $2")
    } else {
        format!("(wl.package = $2 OR ({PKG_KEY}) LIKE $2 || '(%')")
    };

    let base_select = format!(
        "SELECT TO_CHAR(o.created_at AT TIME ZONE 'Asia/Bangkok', 'YYYY-MM-DD HH24:MI:SS') AS created_at,
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
            created_at:   r.get("created_at"),
            lot_id:       r.get("lot_id"),
            package_mpc:  r.get("package_mpc"),
            uph:          0.0,
            bonded_unit:  qty as i64,
            delta_bonded: if with_delta { qty as i64 } else { 0 },
            badge_no:     r.get("badge_no"),
        }
    };

    let current_sql = format!(
        "{base_select} AND o.created_at >= $3::timestamptz AND o.created_at <= $4::timestamptz ORDER BY o.created_at ASC"
    );
    let current: Vec<RawRecord> = sqlx::query(&current_sql)
        .bind(machine_id).bind(package).bind(&w.start).bind(&w.end)
        .fetch_all(pool).await?
        .iter().map(|r| map_row(r, true)).collect();

    let prev_sql = format!(
        "{base_select} AND o.created_at < $3::timestamptz ORDER BY o.created_at DESC LIMIT 5"
    );
    let mut prev_tail: Vec<RawRecord> = sqlx::query(&prev_sql)
        .bind(machine_id).bind(package).bind(&w.start)
        .fetch_all(pool).await?
        .iter().map(|r| map_row(r, false)).collect();
    prev_tail.reverse();

    Ok(json!({ "current": current, "prev_tail": prev_tail }))
}

// ─── 6. Monitor ──────────────────────────────────────────────────────────────
//
// since_min and last_scan_ts are computed in SQL using NOW() and AT TIME ZONE
// to avoid Rust timestamp parsing entirely.

const THRESHOLD_MIN: i64 = 120;

#[derive(Serialize)]
struct MonitorRow {
    machine_id:   String,
    package:      String,
    last_scan_ts: Option<String>,
    since_min:    Option<i64>,
    status:       String,
}

pub async fn query_monitor(pool: &PgPool, date: &str, shift: &str) -> Result<Value> {
    let w  = shift_window(date, shift);
    let as_of = Local::now().naive_local().format("%H:%M").to_string();

    let active_sql = format!(
        "SELECT m.code AS machine_id,
                TO_CHAR(MAX(o.created_at) AT TIME ZONE 'Asia/Bangkok', 'YYYY-MM-DD HH24:MI:SS') AS last_scan_ts,
                ROUND(EXTRACT(EPOCH FROM (NOW() - MAX(o.created_at))) / 60)::int8 AS since_min,
                COALESCE((array_agg(({PKG_KEY}) ORDER BY o.created_at DESC))[1], '') AS package
         FROM output_record o
         JOIN machine m        ON m.id = o.machine_id
         LEFT JOIN wafer_lot wl ON wl.wafer_lot_id = o.wafer_lot_id
         WHERE o.created_at >= $1::timestamptz AND o.created_at <= $2::timestamptz
         GROUP BY m.code"
    );
    let active_rows = sqlx::query(&active_sql).bind(&w.start).bind(&w.end).fetch_all(pool).await?;

    let mut rows: Vec<MonitorRow> = Vec::new();
    let mut active_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in &active_rows {
        let machine_id: String  = r.get("machine_id");
        let last_scan_ts: String = r.get("last_scan_ts");
        let since_min: i64      = r.get("since_min");
        active_ids.insert(machine_id.clone());
        let status = if since_min <= THRESHOLD_MIN { "active" } else { "stale" };
        rows.push(MonitorRow {
            machine_id,
            package: r.get("package"),
            last_scan_ts: Some(last_scan_ts),
            since_min: Some(since_min),
            status: status.to_string(),
        });
    }

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

    let order = |s: &str| match s { "no_data" => 0, "stale" => 1, _ => 2 };
    rows.sort_by(|a, b| {
        let (oa, ob) = (order(&a.status), order(&b.status));
        if oa != ob { oa.cmp(&ob) }
        else { b.since_min.unwrap_or(9999).cmp(&a.since_min.unwrap_or(9999)) }
    });

    Ok(json!({ "rows": rows, "as_of": as_of, "threshold_min": THRESHOLD_MIN }))
}
