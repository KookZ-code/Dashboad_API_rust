// Parity / smoke harness for the WB-UPH repo — calls the ported query functions
// directly against a central.db, no MSSQL pool / server boot required.
//
//   cargo run --example wb_uph_dump -- <db_path> <date> <shift> [hour] [package] [machine_id]
//
// Prints a JSON object { summary, hourly, packages, machines?, records?, monitor }.

use backend::repositories::wb_uph_repo as repo;
use serde_json::json;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let db = a.get(1).expect("db_path required").clone();
    let date = a.get(2).expect("date required").clone();
    let shift = a.get(3).cloned().unwrap_or_else(|| "D".into());
    let hour = a.get(4).and_then(|h| h.parse::<u32>().ok());
    let package = a.get(5).cloned();
    let machine_id = a.get(6).cloned();

    let mut out = json!({
        "summary":  repo::query_summary(&db, &date, &shift, &[])?,
        "hourly":   repo::query_hourly(&db, &date, &shift, &[])?,
        "packages": repo::query_packages(&db, &date, &shift, hour, &[])?,
        "monitor":  repo::query_monitor(&db, &date, &shift)?,
    });

    if let Some(pkg) = &package {
        out["machines"] = repo::query_machines(&db, &date, &shift, hour, pkg)?;
        if let Some(mid) = &machine_id {
            out["records"] = repo::query_records(&db, &date, &shift, mid, pkg)?;
        }
    }

    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
