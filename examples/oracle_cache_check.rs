// Phase-2 gate: build the OracleCache from .env, load historical + live, and exercise
// the in-memory filters — verifies column mapping + filtering against real Oracle.
//
//   cargo run --example oracle_cache_check

use backend::config::Config;
use backend::oracle::OracleCache;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();
    dotenvy::dotenv().ok();

    let cfg = Config::from_env()?;
    let cache = OracleCache::from_config(&cfg);
    println!("ora_enabled = {}", cache.enabled);

    cache.refresh_historical();
    cache.refresh_live();

    let all = cache.filter_historical(None, None, None, None);
    println!("historical (all ISO/FS): {} rows", all.len());
    if let Some(r) = all.iter().max_by_key(|r| r.datex) {
        println!(
            "  latest: machine={} area={} job={} repair={} wait={} datex={:?} shift={}",
            r.machine_id, r.area, r.job_type, r.repair_min, r.wait_min, r.datex, r.shift_code
        );
    }

    let iso_may = cache.filter_historical(Some(&["ISO".to_string()]), Some("2026-05-01"), Some("2026-05-31"), None);
    let fs_may = cache.filter_historical(Some(&["FS".to_string()]), Some("2026-05-01"), Some("2026-05-31"), None);
    println!("ISO May'26: {} | FS May'26: {} rows", iso_may.len(), fs_may.len());

    let live = cache.live_filtered(None);
    println!("live open (ISO/FS): {} rows", live.len());
    for l in live.iter().take(3) {
        println!("  {} {} job={} status={}", l.machine_id, l.area, l.job_type, l.status);
    }
    Ok(())
}
