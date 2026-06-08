/// Column aliases — ตรงกับ `const C` ใน helpers.ts
pub const C_MID:   &str = "code_machine";
pub const C_AREA:  &str = "id_operation";
pub const C_JT:    &str = "job_type";
pub const C_CAUSE: &str = "cause";
pub const C_SYM:   &str = "des_job";
pub const C_OPR:   &str = "datex";
pub const C_TECH:  &str = "date_ack";
pub const C_END:   &str = "date_close";

pub const DOWN_TYPES:  &[&str] = &["M/C DOWN"];
pub const PM_TYPES:    &[&str] = &["PM"];
pub const LOST_TYPES:  &[&str] = &[
    "SETUP", "SETUP BY OPERATOR", "CONVERT", "CLEAN MOLD",
    "CHANGE CAP", "FACILITY DOWN", "ENGINEERING DOWN",
];

/// WHERE clause พร้อม parameter values ตามลำดับ @p1, @p2, …
pub struct WhereClause {
    /// SQL fragment เริ่มต้นด้วย "WHERE ..."
    pub sql: String,
    /// ค่า parameters ตามลำดับ @p1, @p2, … (String เสมอ — tiberius bind VarChar)
    pub params: Vec<String>,
}


pub struct WhereOpts<'a> {
    pub start:      Option<&'a str>,
    pub end:        Option<&'a str>,
    pub areas:      Option<&'a str>,
    pub shift:      Option<&'a str>,
    pub machine_id: Option<&'a str>,
}

/// Port จาก `buildWhere()` ใน helpers.ts
pub fn build_where(opts: WhereOpts<'_>) -> WhereClause {
    let mut clauses: Vec<String> = vec![
        format!("[{}] IS NOT NULL", C_TECH),
        format!("[{}] > [{}]", C_END, C_TECH),
        format!("[{}] IS NOT NULL AND [{}] != ''", C_AREA, C_AREA),
    ];
    let mut params: Vec<String> = Vec::new();

    if let Some(start) = opts.start {
        clauses.push(format!("[{}] >= @p{}", C_OPR, params.len() + 1));
        params.push(start.to_string());
    }
    if let Some(end) = opts.end {
        clauses.push(format!("[{}] < DATEADD(DAY, 1, CAST(@p{} AS DATE))", C_OPR, params.len() + 1));
        params.push(end.to_string());
    }
    if let Some(areas_str) = opts.areas {
        let area_list = parse_areas(areas_str);
        if !area_list.is_empty() {
            let start_idx = params.len() + 1;
            let phs: Vec<String> = (start_idx..start_idx + area_list.len()).map(|i| format!("@p{}", i)).collect();
            clauses.push(format!("[{}] IN ({})", C_AREA, phs.join(", ")));
            params.extend(area_list);
        }
    }
    match opts.shift.map(|s| s.to_uppercase()).as_deref() {
        Some("DAY")   => clauses.push(format!("DATEPART(HOUR, [{}]) BETWEEN 7 AND 18", C_OPR)),
        Some("NIGHT") => clauses.push(format!("DATEPART(HOUR, [{}]) NOT BETWEEN 7 AND 18", C_OPR)),
        _ => {}
    }
    if let Some(mid) = opts.machine_id {
        clauses.push(format!("RTRIM(LTRIM([{}])) = @p{}", C_MID, params.len() + 1));
        params.push(mid.to_string());
    }

    WhereClause { sql: format!("WHERE {}", clauses.join(" AND ")), params }
}

pub struct TechWhereOpts<'a> {
    pub start:    Option<&'a str>,
    pub end:      Option<&'a str>,
    pub areas:    Option<&'a str>,
    pub shift:    Option<&'a str>,
    pub job_type: Option<&'a str>,
}

/// Port จาก `buildTechWhere()` ใน helpers.ts
pub fn build_tech_where(opts: TechWhereOpts<'_>) -> WhereClause {
    let mut clauses: Vec<String> = vec![
        "[date_close] IS NOT NULL".into(),
        "[date_ack] IS NOT NULL".into(),
        "[date_close] > [date_ack]".into(),
        "ISNULL(by_perform, by_ack) IS NOT NULL".into(),
        "ISNULL(by_perform, by_ack) != ''".into(),
    ];
    let mut params: Vec<String> = Vec::new();

    if let Some(start) = opts.start {
        clauses.push(format!("[datex] >= @p{}", params.len() + 1));
        params.push(start.to_string());
    }
    if let Some(end) = opts.end {
        clauses.push(format!("[datex] < DATEADD(DAY, 1, CAST(@p{} AS DATE))", params.len() + 1));
        params.push(end.to_string());
    }
    if let Some(areas_str) = opts.areas {
        let area_list = parse_areas(areas_str);
        if !area_list.is_empty() {
            let start_idx = params.len() + 1;
            let phs: Vec<String> = (start_idx..start_idx + area_list.len()).map(|i| format!("@p{}", i)).collect();
            clauses.push(format!("[id_operation] IN ({})", phs.join(", ")));
            params.extend(area_list);
        }
    }
    match opts.shift.map(|s| s.to_uppercase()).as_deref() {
        Some("DAY")   => clauses.push("DATEPART(HOUR, [datex]) BETWEEN 7 AND 18".into()),
        Some("NIGHT") => clauses.push("DATEPART(HOUR, [datex]) NOT BETWEEN 7 AND 18".into()),
        _ => {}
    }
    if let Some(jt) = opts.job_type {
        clauses.push(format!("[job_type] = @p{}", params.len() + 1));
        params.push(jt.to_string());
    }

    WhereClause { sql: format!("WHERE {}", clauses.join(" AND ")), params }
}

/// "area1,area2,area3" → Vec<String> กรอง blank
pub fn parse_areas(areas: &str) -> Vec<String> {
    areas.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_where_empty() {
        let wc = build_where(WhereOpts { start: None, end: None, areas: None, shift: None, machine_id: None });
        assert!(wc.sql.starts_with("WHERE"));
        assert!(wc.params.is_empty());
    }

    #[test]
    fn build_where_with_date_range() {
        let wc = build_where(WhereOpts {
            start: Some("2024-01-01"), end: Some("2024-01-31"),
            areas: None, shift: None, machine_id: None,
        });
        assert!(wc.sql.contains("@p1") && wc.sql.contains("@p2"));
        assert_eq!(wc.params.len(), 2);
        assert_eq!(wc.params[0], "2024-01-01");
    }

    #[test]
    fn build_where_areas_correct_index() {
        let wc = build_where(WhereOpts {
            start: Some("2024-01-01"), end: None,
            areas: Some("WB,DA"), shift: None, machine_id: None,
        });
        // start = @p1, areas = @p2, @p3
        assert!(wc.sql.contains("@p2") && wc.sql.contains("@p3"));
        assert_eq!(wc.params.len(), 3);
    }

    #[test]
    fn parse_areas_splits_correctly() {
        assert_eq!(parse_areas("WB, DA, IC"), vec!["WB", "DA", "IC"]);
    }

    #[test]
    fn parse_areas_empty_string() {
        assert!(parse_areas("").is_empty());
    }
}
