// Phase-1 gate: prove the `oracle` crate builds (links ODPI-C) and can connect to the
// Oracle DB using the .env config, then count rows in the historical view.
//
//   cargo run --example oracle_smoke
//
// Requires the Oracle Instant Client bin dir (ORA_CLIENT_LIB) on PATH at runtime.

fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let user = std::env::var("ORA_USER")?;
    let pass = std::env::var("ORA_PASSWORD")?;
    let dsn = std::env::var("ORA_DSN")?;
    let view = std::env::var("ORA_VIEW").unwrap_or_else(|_| "Vw_Asodowntime_2025on".into());

    println!("connecting to {dsn} as {user} ...");
    let conn = oracle::Connection::connect(&user, &pass, &dsn)?;
    println!("connected ✓");

    let total: i64 = conn.query_row_as(&format!("SELECT COUNT(*) FROM {view}"), &[])?;
    println!("{view}: {total} rows");

    // distinct equipment types (to confirm ISOLATE / FORM_SING present)
    let mut stmt = conn.statement(&format!(
        "SELECT EQUIPMENT_TYPE, COUNT(*) FROM {view} GROUP BY EQUIPMENT_TYPE ORDER BY 2 DESC"
    )).build()?;
    let rows = stmt.query(&[])?;
    println!("equipment types:");
    for r in rows {
        let r = r?;
        let et: String = r.get(0)?;
        let c: i64 = r.get(1)?;
        println!("  {et}: {c}");
    }
    Ok(())
}
