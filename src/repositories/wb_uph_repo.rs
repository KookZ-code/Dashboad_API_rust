// WB-UPH module — SQLite `central.db` (hourly bond-unit scan records).
//
// Ports the six query modules from the SvelteKit frontend
// (svelte_frontend/frontend/src/lib/server/queries/*.ts) — which are the current
// source of truth — into Rust. The frontend versions had diverged from the older
// WB_Dashboard/src/db.rs (notably the reset-aware delta logic in delta.ts), so the
// semantics here mirror the TypeScript, NOT WB_Dashboard's original SQL.
//
// PLAN-INDEPENDENCE: per the migration decision the Excel plan stays in the
// frontend, so every function here returns RAW numbers only. Plan targets and
// vs-target percentages are layered on in the frontend's +server.ts.
//
// rusqlite is synchronous — handlers invoke these via tokio::task::spawn_blocking.

use anyhow::Result;
use chrono::{Local, NaiveDate, NaiveDateTime, Timelike};
use rusqlite::{named_params, Connection, OpenFlags};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

// ─── Connection ───────────────────────────────────────────────────────────────
//
// SQLite over an SMB network share is unreliable: WAL/shared-memory isn't
// supported on network filesystems, and the share is transiently locked at shift
// boundaries (the scan collector is writing). So — mirroring the old frontend's
// db-sync — we copy the share file to a local cache (only when its mtime changes)
// and open the cache read-only + immutable. Local paths are opened directly.

static SYNC_MTIME: Mutex<Option<SystemTime>> = Mutex::new(None);
static SYNCING: AtomicBool = AtomicBool::new(false);
static CACHE_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static VERSION: AtomicU64 = AtomicU64::new(0);

fn is_network_share(p: &str) -> bool {
    p.starts_with("\\\\") || p.starts_with("//")
}

fn cache_for(v: u64) -> PathBuf {
    std::env::temp_dir().join(format!("wb_uph_central_{v}.db"))
}

/// Delete versioned caches older than `min_keep` (best-effort). Keeps the most
/// recent few so a request that just captured an older pointer can still open it.
fn sweep_below(min_keep: u64) {
    if let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) {
        for e in rd.flatten() {
            let p = e.path();
            let v = p
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(|n| n.strip_prefix("wb_uph_central_"))
                .and_then(|n| n.strip_suffix(".db"))
                .and_then(|n| n.parse::<u64>().ok());
            if matches!(v, Some(v) if v < min_keep) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

/// Copy share → a new versioned cache and swap the pointer. Returns the new path.
/// Copies to a fresh filename (never rename/overwrite) because on Windows the cache
/// is held open by SQLite readers and overwriting an open file fails.
fn copy_and_swap(src: &Path, mtime: Option<SystemTime>) -> Result<PathBuf> {
    let v = VERSION.fetch_add(1, Ordering::SeqCst) + 1;
    let dst = cache_for(v);
    std::fs::copy(src, &dst).map_err(|e| anyhow::anyhow!("mirror central.db failed: {e}"))?;
    *CACHE_PATH.lock().unwrap() = Some(dst.clone());
    *SYNC_MTIME.lock().unwrap() = mtime;
    sweep_below(v.saturating_sub(2)); // keep current + previous two
    Ok(dst)
}

/// Resolve the path to open. For a network share: serve the current local cache
/// IMMEDIATELY and, if the share is newer, refresh in the background (stale-while-
/// revalidate). The very first copy is single-flighted and blocks until ready.
fn resolve_db(db_path: &str) -> Result<PathBuf> {
    if !is_network_share(db_path) {
        return Ok(PathBuf::from(db_path));
    }
    let src = Path::new(db_path);
    // A metadata stat over SMB is cheap (unlike copying the whole file).
    let src_mtime = std::fs::metadata(src).and_then(|m| m.modified()).ok();
    let current = CACHE_PATH.lock().unwrap().clone();

    let changed = {
        let last = *SYNC_MTIME.lock().unwrap();
        match (src_mtime, last) {
            (Some(c), Some(p)) => c != p,
            (Some(_), None) => true,
            _ => false, // share unreachable → keep serving the cache
        }
    };

    if (current.is_none() || changed)
        && SYNCING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
    {
        if current.is_none() {
            // First copy — block this request so it has data, then release the guard.
            let r = copy_and_swap(src, src_mtime);
            SYNCING.store(false, Ordering::SeqCst);
            return r;
        }
        // Newer share — refresh in the background; serve `current` now.
        let src_buf = src.to_path_buf();
        let target = src_mtime;
        std::thread::spawn(move || {
            let _ = copy_and_swap(&src_buf, target);
            SYNCING.store(false, Ordering::SeqCst);
        });
    }

    if let Some(cur) = current {
        return Ok(cur);
    }
    // No cache yet and another thread is doing the initial copy — wait for it.
    // Timeout 60s: large central.db over a slow SMB share can take >6s to copy.
    for _ in 0..1200 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if let Some(c) = CACHE_PATH.lock().unwrap().clone() {
            return Ok(c);
        }
    }
    Err(anyhow::anyhow!("central.db mirror not ready — share may be unreachable or file too large"))
}

/// Kick off the initial central.db copy at server startup so the cache is warm
/// before the first request. Call via spawn_blocking — non-blocking from async.
pub fn warmup(db_path: &str) {
    match resolve_db(db_path) {
        Ok(_)  => tracing::info!("central.db mirror ready"),
        Err(e) => tracing::warn!("central.db warmup failed (WB-UPH will retry on first request): {e}"),
    }
}

fn open(db_path: &str) -> Result<Connection> {
    let path = resolve_db(db_path)?;
    // Read-only + immutable: treat the (stable, locally-cached) file as unchanging
    // so SQLite skips locking and -wal/-shm — works regardless of source journal mode.
    let uri = format!(
        "file:///{}?immutable=1&mode=ro",
        path.to_string_lossy().replace('\\', "/").trim_start_matches('/').replace(' ', "%20")
    );
    let conn = Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )?;
    Ok(conn)
}

// ─── Shift window (port of shift.ts) ────────────────────────────────────────────

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

/// Mirror of handler-utils.resolveShift + currentShift — used to fill defaults so
/// direct API calls behave like the old frontend. The frontend normally sends an
/// explicit, already-validated (date, shift).
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

// ─── Shared SQL / delta helpers (ports of db.ts + delta.ts) ─────────────────────

/// Filter clause matching either base `package` or the mpc-derived key.
/// Values come from our own dropdown (frontend already maps display→db), so inline
/// string-concat with quote-escaping matches the original.
fn build_pkg_clause(filter: &[String]) -> String {
    if filter.is_empty() {
        return String::new();
    }
    let list = filter
        .iter()
        .map(|p| format!("'{}'", p.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "AND (package IN ({list}) OR \
              COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) IN ({list}))"
    )
}

/// Production for one scan given the previous cumulative counter value.
/// A decrease means a capillary reset → the post-reset value is itself production.
fn scan_delta(prev: i64, cur: i64) -> i64 {
    if cur >= prev { cur - prev } else { cur }
}

fn reset_aware_total(baseline: i64, values: &[i64]) -> i64 {
    let mut total = 0;
    let mut prev = baseline;
    for &v in values {
        total += scan_delta(prev, v);
        prev = v;
    }
    total
}

/// Latest bonded_unit per (machine_id, lot_id) before shift start.
fn load_pre_baselines(conn: &Connection, start: &str) -> Result<HashMap<(String, String), i64>> {
    let mut stmt = conn.prepare(
        "SELECT machine_id, lot_id, bonded_unit
         FROM uph_records
         WHERE voided = 0 AND created_at < ?1
         ORDER BY machine_id, lot_id, created_at DESC",
    )?;
    let rows = stmt
        .query_map([start], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .filter_map(|r| r.ok());
    let mut map: HashMap<(String, String), i64> = HashMap::new();
    for (machine_id, lot_id, bonded_unit) in rows {
        map.entry((machine_id, lot_id)).or_insert(bonded_unit);
    }
    Ok(map)
}

/// Carry-over baseline when no pre-shift record exists: if the first scan's implied
/// UPH is more than 2× the reported UPH, treat the first bonded value as carried in.
fn carryover_baseline(start_secs: i64, first_ts: &str, first_bonded: i64, first_uph: f64) -> i64 {
    let elapsed_h = (parse_ts_secs(first_ts) - start_secs) as f64 / 3600.0;
    if elapsed_h > 0.0 && first_uph > 0.0 {
        let implied = first_bonded as f64 / elapsed_h;
        if implied > first_uph * 2.0 { first_bonded } else { 0 }
    } else {
        0
    }
}

// ─── 1. Summary (port of summary.ts) ────────────────────────────────────────────

pub fn query_summary(db_path: &str, date: &str, shift: &str, pkg_filter: &[String]) -> Result<Value> {
    let conn = open(db_path)?;
    let w = shift_window(date, shift);
    let pre = load_pre_baselines(&conn, &w.start)?;

    let pkg_clause = build_pkg_clause(pkg_filter);
    let sql = format!(
        "SELECT machine_id, lot_id, bonded_unit, created_at, uph, COALESCE(badge_no, '') AS badge_no
         FROM uph_records
         WHERE voided = 0 AND created_at >= :start AND created_at <= :end {pkg_clause}
         ORDER BY created_at"
    );

    struct Group {
        machine: String,
        bonded: Vec<i64>,
        first_ts: String,
        first_bonded: i64,
        first_uph: f64,
    }
    let mut groups: HashMap<(String, String), Group> = HashMap::new();
    let mut operators: HashSet<String> = HashSet::new();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(named_params! { ":start": w.start, ":end": w.end }, |r| {
            Ok((
                r.get::<_, String>(0)?, // machine_id
                r.get::<_, String>(1)?, // lot_id
                r.get::<_, i64>(2)?,    // bonded_unit
                r.get::<_, String>(3)?, // created_at
                r.get::<_, f64>(4)?,    // uph
                r.get::<_, String>(5)?, // badge_no
            ))
        })?
        .filter_map(|r| r.ok());

    for (machine_id, lot_id, bonded_unit, created_at, uph, badge_no) in rows {
        if !badge_no.is_empty() {
            operators.insert(badge_no);
        }
        let g = groups
            .entry((machine_id.clone(), lot_id))
            .or_insert_with(|| Group {
                machine: machine_id,
                bonded: Vec::new(),
                first_ts: created_at.clone(),
                first_bonded: bonded_unit,
                first_uph: uph,
            });
        g.bonded.push(bonded_unit);
    }

    let start_secs = parse_ts_secs(&w.start);
    let mut total_bonded: i64 = 0;
    let mut machines: HashSet<String> = HashSet::new();

    for ((machine, lot), g) in &groups {
        let baseline = match pre.get(&(machine.clone(), lot.clone())) {
            Some(&pv) => pv,
            None => carryover_baseline(start_secs, &g.first_ts, g.first_bonded, g.first_uph),
        };
        total_bonded += reset_aware_total(baseline, &g.bonded);
        machines.insert(g.machine.clone());
    }

    Ok(json!({
        "total_bonded": total_bonded,
        "active_machines": machines.len() as i64,
        "active_operators": operators.len() as i64,
    }))
}

// ─── 2. Hourly (port of hourly.ts) ──────────────────────────────────────────────

pub fn query_hourly(db_path: &str, date: &str, shift: &str, pkg_filter: &[String]) -> Result<Value> {
    let conn = open(db_path)?;
    let w = shift_window(date, shift);
    let pre = load_pre_baselines(&conn, &w.start)?;

    let pkg_clause = build_pkg_clause(pkg_filter);
    let sql = format!(
        "SELECT COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) AS pkg_key,
                machine_id, lot_id, bonded_unit, created_at, uph
         FROM uph_records
         WHERE voided = 0 AND created_at >= :start AND created_at <= :end {pkg_clause}
         ORDER BY created_at"
    );

    // (pkg, machine, lot) → time-ordered ts + bonded; plus first-scan info.
    type Key = (String, String, String);
    struct Series {
        pkg: String,
        machine: String,
        lot: String,
        ts: Vec<String>,
        bonded: Vec<i64>,
    }
    let mut series: HashMap<Key, Series> = HashMap::new();
    let mut first_scan: HashMap<Key, (i64, String, f64)> = HashMap::new();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(named_params! { ":start": w.start, ":end": w.end }, |r| {
            Ok((
                r.get::<_, String>(0)?.trim().to_string(), // pkg_key
                r.get::<_, String>(1)?,                    // machine_id
                r.get::<_, String>(2)?,                    // lot_id
                r.get::<_, i64>(3)?,                        // bonded_unit
                r.get::<_, String>(4)?,                    // created_at
                r.get::<_, f64>(5)?,                        // uph
            ))
        })?
        .filter_map(|r| r.ok());

    for (pkg, machine, lot, bonded, ts, uph) in rows {
        let key = (pkg.clone(), machine.clone(), lot.clone());
        first_scan
            .entry(key.clone())
            .or_insert((bonded, ts.clone(), uph));
        let s = series.entry(key).or_insert_with(|| Series {
            pkg,
            machine,
            lot,
            ts: Vec::new(),
            bonded: Vec::new(),
        });
        s.ts.push(ts);
        s.bonded.push(bonded);
    }

    let start_secs = parse_ts_secs(&w.start);
    let n = w.hours.len();
    let mut pkg_map: HashMap<String, Vec<i64>> = HashMap::new();

    for (slot_idx, &h) in w.hours.iter().enumerate() {
        let slot_end = slot_end_for_hour(&w, h);
        let mut slot_totals: HashMap<String, i64> = HashMap::new();

        for (key, s) in &series {
            // bonded values scanned up to slot_end (ts ascending → break early)
            let mut vals: Vec<i64> = Vec::new();
            for (i, t) in s.ts.iter().enumerate() {
                if t.as_str() <= slot_end.as_str() {
                    vals.push(s.bonded[i]);
                } else {
                    break;
                }
            }
            if vals.is_empty() {
                continue;
            }
            let baseline = match pre.get(&(s.machine.clone(), s.lot.clone())) {
                Some(&pv) => pv,
                None => {
                    let (fb, fts, fuph) = &first_scan[key];
                    carryover_baseline(start_secs, fts, *fb, *fuph)
                }
            };
            let delta = reset_aware_total(baseline, &vals);
            *slot_totals.entry(s.pkg.clone()).or_insert(0) += delta;
        }

        for (pkg, total) in slot_totals {
            let arr = pkg_map.entry(pkg).or_insert_with(|| vec![0; n]);
            arr[slot_idx] = total;
        }
    }

    Ok(json!({ "packages": pkg_map }))
}

// ─── 3. Packages (port of packages.ts — SQL MAX-baseline, raw rows only) ─────────

#[derive(Serialize)]
struct PackageRaw {
    package: String,
    bonded: i64,
}

pub fn query_packages(
    db_path: &str,
    date: &str,
    shift: &str,
    hour: Option<u32>,
    pkg_filter: &[String],
) -> Result<Value> {
    let w = shift_window(date, shift);
    let hour = hour.unwrap_or_else(|| *w.hours.last().unwrap_or(&18));
    let slot_end = slot_end_for_hour(&w, hour);

    let conn = open(db_path)?;
    let pkg_clause = build_pkg_clause(pkg_filter);
    let sql = format!(
        "SELECT pkg_key, COALESCE(SUM(delta), 0) AS bonded
         FROM (
             SELECT COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) AS pkg_key, machine_id, lot_id,
                    MAX(0, MAX(bonded_unit) - COALESCE(
                        (SELECT bonded_unit FROM uph_records pre
                         WHERE pre.machine_id = main.machine_id
                           AND pre.lot_id     = main.lot_id
                           AND pre.voided     = 0
                           AND pre.created_at < :start
                         ORDER BY pre.created_at DESC LIMIT 1),
                        (SELECT
                            CASE
                                WHEN (julianday(f.created_at) - julianday(:start)) > 0
                                 AND f.bonded_unit /
                                     ((julianday(f.created_at) - julianday(:start)) * 24.0) > f.uph * 2
                                THEN f.bonded_unit
                                ELSE 0
                            END
                         FROM uph_records f
                         WHERE f.machine_id = main.machine_id
                           AND f.lot_id     = main.lot_id
                           AND f.voided     = 0
                           AND f.created_at >= :start
                           AND f.created_at <= :slot_end
                         ORDER BY f.created_at ASC LIMIT 1)
                    )) AS delta
             FROM uph_records main
             WHERE voided = 0 AND created_at >= :start AND created_at <= :slot_end {pkg_clause}
             GROUP BY COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END), machine_id, lot_id
         )
         GROUP BY pkg_key
         ORDER BY bonded DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows: Vec<PackageRaw> = stmt
        .query_map(
            named_params! { ":start": w.start, ":slot_end": slot_end },
            |r| {
                Ok(PackageRaw {
                    package: r.get::<_, String>(0)?,
                    bonded: r.get::<_, i64>(1)?,
                })
            },
        )?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::to_value(rows)?)
}

// ─── 4. Machines (port of machines.ts — reset-aware, raw rows only) ──────────────

#[derive(Serialize)]
struct MachineRaw {
    machine_id: String,
    badge_no: String,
    uph: f64,
    bonded_unit: i64,
    last_scan_ts: Option<String>,
    pkg_mpc: String,
}

pub fn query_machines(
    db_path: &str,
    date: &str,
    shift: &str,
    hour: Option<u32>,
    package: &str,
) -> Result<Value> {
    let w = shift_window(date, shift);
    let hour = hour.unwrap_or_else(|| *w.hours.last().unwrap_or(&18));
    let slot_end = slot_end_for_hour(&w, hour);

    let conn = open(db_path)?;
    let pre = load_pre_baselines(&conn, &w.start)?;

    // Base key (no parens) must catch MPC variants but not space-qualified variants.
    let pkg_clause = if package.contains('(') {
        "AND (COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) = :pkg)".to_string()
    } else {
        "AND (COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) = :pkg
              OR (package = :pkg AND (package_mpc IS NULL OR package_mpc LIKE :pkg || '(%')))".to_string()
    };

    let sql = format!(
        "SELECT machine_id, lot_id, bonded_unit, created_at, uph,
                COALESCE(badge_no, '') AS badge_no,
                COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9
                         THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) AS pkg_mpc
         FROM uph_records
         WHERE voided = 0 AND created_at >= :start AND created_at <= :slot_end {pkg_clause}
         ORDER BY created_at"
    );

    struct Raw {
        machine_id: String,
        lot_id: String,
        bonded_unit: i64,
        created_at: String,
        uph: f64,
        badge_no: String,
        pkg_mpc: String,
    }
    let mut stmt = conn.prepare(&sql)?;
    let raw: Vec<Raw> = stmt
        .query_map(
            named_params! { ":start": w.start, ":slot_end": slot_end, ":pkg": package },
            |r| {
                Ok(Raw {
                    machine_id: r.get(0)?,
                    lot_id: r.get(1)?,
                    bonded_unit: r.get(2)?,
                    created_at: r.get(3)?,
                    uph: r.get(4)?,
                    badge_no: r.get(5)?,
                    pkg_mpc: r.get(6)?,
                })
            },
        )?
        .filter_map(|r| r.ok())
        .collect();

    // Per-(machine, lot) bonded series (time-ordered)
    struct Series {
        machine: String,
        lot: String,
        bonded: Vec<i64>,
        first_ts: String,
        first_bonded: i64,
        first_uph: f64,
    }
    let mut series: HashMap<String, Series> = HashMap::new();
    for r in &raw {
        let key = format!("{}\0{}", r.machine_id, r.lot_id);
        let s = series.entry(key).or_insert_with(|| Series {
            machine: r.machine_id.clone(),
            lot: r.lot_id.clone(),
            bonded: Vec::new(),
            first_ts: r.created_at.clone(),
            first_bonded: r.bonded_unit,
            first_uph: r.uph,
        });
        s.bonded.push(r.bonded_unit);
    }

    let start_secs = parse_ts_secs(&w.start);

    // Aggregate per machine: reset-aware bonded sum, then latest uph/badge/pkg_mpc/last_scan.
    struct Agg {
        bonded: i64,
        latest_uph: f64,
        last_scan_ts: String,
        badge: String,
        pkg_mpc: String,
    }
    let mut agg: HashMap<String, Agg> = HashMap::new();

    for s in series.values() {
        let baseline = match pre.get(&(s.machine.clone(), s.lot.clone())) {
            Some(&pv) => pv,
            None => carryover_baseline(start_secs, &s.first_ts, s.first_bonded, s.first_uph),
        };
        let delta = reset_aware_total(baseline, &s.bonded);
        agg.entry(s.machine.clone())
            .and_modify(|a| a.bonded += delta)
            .or_insert(Agg {
                bonded: delta,
                latest_uph: 0.0,
                last_scan_ts: String::new(),
                badge: String::new(),
                pkg_mpc: String::new(),
            });
    }

    // raw is time-ordered → last wins for uph/badge/pkg_mpc; max for last_scan_ts.
    for r in &raw {
        if let Some(a) = agg.get_mut(&r.machine_id) {
            if !r.badge_no.is_empty() {
                a.badge = r.badge_no.clone();
            }
            if !r.pkg_mpc.is_empty() {
                a.pkg_mpc = r.pkg_mpc.clone();
            }
            if r.created_at > a.last_scan_ts {
                a.last_scan_ts = r.created_at.clone();
            }
            if r.uph > 0.0 {
                a.latest_uph = r.uph;
            }
        }
    }

    let mut out: Vec<MachineRaw> = agg
        .into_iter()
        .map(|(machine_id, a)| MachineRaw {
            machine_id,
            badge_no: a.badge,
            uph: a.latest_uph,
            bonded_unit: a.bonded,
            last_scan_ts: if a.last_scan_ts.is_empty() { None } else { Some(a.last_scan_ts) },
            pkg_mpc: a.pkg_mpc,
        })
        .collect();
    out.sort_by(|a, b| b.bonded_unit.cmp(&a.bonded_unit));

    Ok(serde_json::to_value(out)?)
}

// ─── 5. Records (port of records.ts — fully plan-independent) ────────────────────

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

fn map_raw_record(r: &rusqlite::Row) -> rusqlite::Result<RawRecord> {
    Ok(RawRecord {
        created_at: r.get(0)?,
        lot_id: r.get(1)?,
        package_mpc: r.get::<_, String>(2)?.trim().to_string(),
        uph: r.get(3)?,
        bonded_unit: r.get(4)?,
        delta_bonded: r.get(5)?,
        badge_no: r.get(6)?,
    })
}

pub fn query_records(
    db_path: &str,
    date: &str,
    shift: &str,
    machine_id: &str,
    package: &str,
) -> Result<Value> {
    let conn = open(db_path)?;
    let w = shift_window(date, shift);

    // current shift — LAG() previous cumulative, then reset-aware delta.
    let current_sql = "SELECT
             created_at, lot_id, pkg, uph, bonded_unit,
             CASE WHEN bonded_unit >= prevval THEN bonded_unit - prevval ELSE bonded_unit END AS delta_bonded,
             badge_no
         FROM (
             SELECT
                 created_at, lot_id,
                 COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) AS pkg,
                 uph, bonded_unit,
                 COALESCE(
                     LAG(bonded_unit) OVER (PARTITION BY lot_id ORDER BY created_at),
                     (SELECT bonded_unit FROM uph_records pre2
                      WHERE pre2.machine_id = :machine
                        AND pre2.lot_id     = uph_records.lot_id
                        AND pre2.voided     = 0
                        AND pre2.created_at < :start
                      ORDER BY pre2.created_at DESC LIMIT 1),
                     CASE
                         WHEN (julianday(created_at) - julianday(:start)) > 0
                          AND bonded_unit / ((julianday(created_at) - julianday(:start)) * 24.0) > uph * 2
                         THEN bonded_unit ELSE 0
                     END
                 ) AS prevval,
                 COALESCE(badge_no, '') AS badge_no
             FROM uph_records
             WHERE voided = 0 AND machine_id = :machine
               AND (COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) = :pkg
                    OR (machine_id = :machine AND package = :pkg
                        AND (package_mpc IS NULL OR package_mpc LIKE :pkg || '(%')))
               AND created_at >= :start AND created_at <= :end
         )
         ORDER BY created_at ASC";

    let mut stmt = conn.prepare(current_sql)?;
    let current: Vec<RawRecord> = stmt
        .query_map(
            named_params! { ":machine": machine_id, ":pkg": package, ":start": w.start, ":end": w.end },
            map_raw_record,
        )?
        .filter_map(|r| r.ok())
        .collect();

    // previous-shift tail — last 5 scans before shift start, oldest first.
    let prev_sql = "SELECT created_at, lot_id,
              COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) AS pkg,
              uph, bonded_unit, 0 AS delta_bonded, COALESCE(badge_no, '') AS badge_no
       FROM uph_records
       WHERE voided = 0 AND machine_id = :machine
         AND (COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END) = :pkg
              OR (package = :pkg AND (package_mpc IS NULL OR package_mpc LIKE :pkg || '(%')))
         AND created_at < :start
       ORDER BY created_at DESC
       LIMIT :limit";

    let mut stmt = conn.prepare(prev_sql)?;
    let mut prev_tail: Vec<RawRecord> = stmt
        .query_map(
            named_params! { ":machine": machine_id, ":pkg": package, ":start": w.start, ":limit": 5_i64 },
            map_raw_record,
        )?
        .filter_map(|r| r.ok())
        .collect();
    prev_tail.reverse(); // oldest first

    Ok(json!({ "current": current, "prev_tail": prev_tail }))
}

// ─── 6. Monitor (port of monitor.ts) ─────────────────────────────────────────────

const THRESHOLD_MIN: i64 = 120;

#[derive(Serialize)]
struct MonitorRow {
    machine_id: String,
    package: String,
    last_scan_ts: Option<String>,
    since_min: Option<i64>,
    status: String,
}

pub fn query_monitor(db_path: &str, date: &str, shift: &str) -> Result<Value> {
    let conn = open(db_path)?;
    let w = shift_window(date, shift);

    let now = Local::now().naive_local();
    let now_ts = now.format("%Y-%m-%d %H:%M:%S").to_string();
    let as_of = now.format("%H:%M").to_string();
    let now_min = parse_ts_secs(&now_ts) / 60;

    // 1. machines that scanned this shift
    let active_sql = "SELECT machine_id, MAX(created_at) AS last_scan_ts,
               COALESCE(
                   (SELECT COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END)
                    FROM uph_records r2
                    WHERE r2.machine_id = r1.machine_id AND r2.voided = 0
                      AND r2.created_at >= :start AND r2.created_at <= :end
                    ORDER BY r2.created_at DESC LIMIT 1), '') AS package
        FROM uph_records r1
        WHERE voided = 0 AND created_at >= :start AND created_at <= :end
        GROUP BY machine_id";
    let mut stmt = conn.prepare(active_sql)?;
    let active: Vec<(String, String, String)> = stmt
        .query_map(named_params! { ":start": w.start, ":end": w.end }, |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    let active_ids: HashSet<String> = active.iter().map(|(m, _, _)| m.clone()).collect();

    // 2. recently-active machines that have NOT scanned this shift
    let lookback = format!("{} 00:00:00", &w.start[..10]);
    let nodata_sql = "SELECT machine_id,
               COALESCE(
                   (SELECT COALESCE(package_mpc, CASE WHEN mpc IS NOT NULL AND LENGTH(mpc)>=9 THEN package||'('||SUBSTR(mpc,7,3)||')' ELSE package END)
                    FROM uph_records r2
                    WHERE r2.machine_id = r1.machine_id AND r2.voided = 0
                    ORDER BY r2.created_at DESC LIMIT 1), '') AS package
        FROM uph_records r1
        WHERE voided = 0 AND created_at >= :lookback AND created_at < :start
        GROUP BY machine_id";
    let mut stmt = conn.prepare(nodata_sql)?;
    let nodata: Vec<(String, String)> = stmt
        .query_map(named_params! { ":lookback": lookback, ":start": w.start }, |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut rows: Vec<MonitorRow> = Vec::new();
    for (machine_id, last_scan_ts, package) in active {
        let since_min = now_min - parse_ts_secs(&last_scan_ts) / 60;
        let status = if since_min <= THRESHOLD_MIN { "active" } else { "stale" };
        rows.push(MonitorRow {
            machine_id,
            package,
            last_scan_ts: Some(last_scan_ts),
            since_min: Some(since_min),
            status: status.to_string(),
        });
    }
    for (machine_id, package) in nodata {
        if active_ids.contains(&machine_id) {
            continue;
        }
        rows.push(MonitorRow {
            machine_id,
            package,
            last_scan_ts: None,
            since_min: None,
            status: "no_data".to_string(),
        });
    }

    // Sort: no_data → stale → active (worst first); within group, most stale first.
    let order = |s: &str| match s {
        "no_data" => 0,
        "stale" => 1,
        _ => 2,
    };
    rows.sort_by(|a, b| {
        let oa = order(&a.status);
        let ob = order(&b.status);
        if oa != ob {
            oa.cmp(&ob)
        } else {
            b.since_min.unwrap_or(9999).cmp(&a.since_min.unwrap_or(9999))
        }
    });

    Ok(json!({ "rows": rows, "as_of": as_of, "threshold_min": THRESHOLD_MIN }))
}
