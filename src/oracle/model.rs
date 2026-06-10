// Normalized Oracle rows for ISO/FS areas — mirrors the column mapping in the
// original Python oracle_db.py (Oracle column → dashboard field).

use chrono::NaiveDateTime;

/// EQUIPMENT_TYPE (Oracle) → dashboard area code.
pub fn area_from_equipment_type(et: &str) -> Option<&'static str> {
    match et {
        "ISOLATE" => Some("ISO"),
        "FORM_SING" => Some("FS"),
        _ => None, // e.g. ROBOT — not an ISO/FS area, ignored
    }
}

/// Historical downtime/job record (from Vw_Asodowntime_2025on).
#[derive(Debug, Clone)]
pub struct OracleRow {
    pub machine_id: String,            // EQUIPMENT_ID
    pub datex: Option<NaiveDateTime>,  // S_DATE
    pub date_ack: Option<NaiveDateTime>,   // P_START
    pub date_close: Option<NaiveDateTime>, // P_STOP
    pub job_type: String,              // JOB_TYPE
    pub cause: String,                 // CAUSE
    pub symptom: String,               // CRITERIA
    pub repair_min: f64,               // DOWNTIME
    pub wait_min: f64,                 // WAIT_TECH
    pub badge: String,                 // BADGE_NO
    pub package_type: String,          // PKG
    pub lot_no: String,                // LOT_ID
    pub die_mask: String,              // PRODUCT_ID
    pub action: String,                // TECHNICIAN_COMMENT
    pub area: String,                  // from EQUIPMENT_TYPE (ISO/FS)
    pub shift_code: String,            // SHIFT (D/N)
}

/// Live status row (from EQ_USER.V_EQDOWNTIME) for the Overview / open-jobs pages.
#[derive(Debug, Clone)]
pub struct OracleLiveRow {
    pub machine_id: String,            // EQUIPMENT_ID
    pub area: String,                  // ISO/FS
    pub job_type: String,              // derived from CAUSE/CRITERIA
    pub des_job: String,               // CRITERIA
    pub status: String,                // On Process / Closed / Waiting
    pub date_close: Option<NaiveDateTime>, // P_STOP
    pub wait_min: i64,                 // WAIT_TECH
    pub repair_min: i64,               // DOWNTIME
    pub badge: String,                 // BADGE_NO
    pub package_type: String,          // PKG
    pub lot_no: String,                // LOT_ID
    pub die_mask: String,              // PRODUCT_ID
}
