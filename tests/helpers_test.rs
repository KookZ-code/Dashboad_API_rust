#[cfg(test)]
mod where_builder_tests {
    use backend::helpers::where_builder::*;

    #[test]
    fn empty_where_has_base_clauses() {
        let wc = build_where(WhereOpts { start: None, end: None, areas: None, shift: None, machine_id: None });
        assert!(wc.sql.contains("date_ack] IS NOT NULL"), "missing date_ack IS NOT NULL");
        assert!(wc.sql.contains("date_close] > [date_ack]"), "missing date_close > date_ack");
        assert!(wc.sql.contains("id_operation] IS NOT NULL"), "missing id_operation IS NOT NULL");
        assert!(wc.params.is_empty());
    }

    #[test]
    fn date_range_adds_two_params() {
        let wc = build_where(WhereOpts {
            start: Some("2024-01-01"), end: Some("2024-12-31"),
            areas: None, shift: None, machine_id: None,
        });
        assert_eq!(wc.params.len(), 2);
        assert_eq!(wc.params[0], "2024-01-01");
        assert_eq!(wc.params[1], "2024-12-31");
        assert!(wc.sql.contains("@p1"));
        assert!(wc.sql.contains("@p2"));
    }

    #[test]
    fn areas_param_indices_follow_dates() {
        let wc = build_where(WhereOpts {
            start: Some("2024-01-01"), end: None,
            areas: Some("WB,DA,IC"), shift: None, machine_id: None,
        });
        // start = @p1, areas = @p2, @p3, @p4
        assert_eq!(wc.params.len(), 4);
        assert!(wc.sql.contains("@p2") && wc.sql.contains("@p3") && wc.sql.contains("@p4"));
        assert_eq!(wc.params[1], "WB");
        assert_eq!(wc.params[2], "DA");
        assert_eq!(wc.params[3], "IC");
    }

    #[test]
    fn shift_day_adds_hour_clause() {
        let wc = build_where(WhereOpts { start: None, end: None, areas: None, shift: Some("day"), machine_id: None });
        assert!(wc.sql.to_uppercase().contains("BETWEEN 7 AND 18"));
        assert!(wc.params.is_empty());
    }

    #[test]
    fn shift_night_adds_not_between() {
        let wc = build_where(WhereOpts { start: None, end: None, areas: None, shift: Some("NIGHT"), machine_id: None });
        assert!(wc.sql.to_uppercase().contains("NOT BETWEEN 7 AND 18"));
    }

    #[test]
    fn machine_id_appended_last() {
        let wc = build_where(WhereOpts {
            start: Some("2024-01-01"), end: None, areas: None, shift: None,
            machine_id: Some("MC001"),
        });
        assert_eq!(wc.params.len(), 2);
        assert_eq!(wc.params.last().unwrap(), "MC001");
    }

    #[test]
    fn tech_where_empty() {
        let wc = build_tech_where(TechWhereOpts { start: None, end: None, areas: None, shift: None, job_type: None });
        assert!(wc.sql.contains("date_close] IS NOT NULL"));
        assert!(wc.params.is_empty());
    }

    #[test]
    fn parse_areas_trims_whitespace() {
        let v = parse_areas("  WB , DA , IC  ");
        assert_eq!(v, vec!["WB", "DA", "IC"]);
    }

    #[test]
    fn parse_areas_filters_empty() {
        let v = parse_areas("WB,,DA");
        assert_eq!(v, vec!["WB", "DA"]);
    }
}

#[cfg(test)]
mod kpi_tests {
    use backend::helpers::kpi::*;

    #[test]
    fn r2_basic() {
        assert_eq!(r2(85.336), 85.34);
        assert_eq!(r2(0.0),    0.0);
        assert_eq!(r2(100.0),  100.0);
    }

    #[test]
    fn n_days_single_day() {
        assert_eq!(n_days(Some("2024-06-01"), Some("2024-06-01")), 1);
    }

    #[test]
    fn n_days_full_month() {
        assert_eq!(n_days(Some("2024-01-01"), Some("2024-01-31")), 31);
    }

    #[test]
    fn n_days_default_when_none() {
        assert_eq!(n_days(None, None), 30);
        assert_eq!(n_days(Some("2024-01-01"), None), 30);
    }

    #[test]
    fn compute_kpis_no_downtime() {
        let kpi = compute_kpis(&[], 10, 7, None);
        assert_eq!(kpi.utilization_pct, 100.0);
        assert_eq!(kpi.downtime_pct,    0.0);
        assert_eq!(kpi.pm_pct,          0.0);
        assert_eq!(kpi.lost_time_pct,   0.0);
    }

    #[test]
    fn compute_kpis_full_downtime_one_machine() {
        // 1 machine, 1 day (24h = 1440 min), job_type = M/C DOWN for 1440 min
        let rows = vec![KpiRow { job_type: "M/C DOWN".into(), total_min: 1440.0, wait_min: 0.0 }];
        let kpi = compute_kpis(&rows, 1, 1, None);
        assert_eq!(kpi.downtime_pct,    100.0);
        assert_eq!(kpi.utilization_pct, 0.0);
    }

    #[test]
    fn area_util_single_area() {
        let area_rows = vec![AreaRow { area: "WB".into(), job_type: "M/C DOWN".into(), total_min: 0.0, wait_min: 0.0 }];
        let mc_rows   = vec![McCountRow { area: "WB".into(), machine_count: 5 }];
        let result = area_util(&area_rows, &mc_rows, 1, None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].area, "WB");
        assert_eq!(result[0].utilization_pct, 100.0);
        assert_eq!(result[0].target_pct, 85.0);
    }

    #[test]
    fn monthly_trend_sorts_by_month() {
        let rows = vec![
            MonthlyRow { ym: "2024-03".into(), job_type: "M/C DOWN".into(), total_min: 0.0, wait_min: 0.0 },
            MonthlyRow { ym: "2024-01".into(), job_type: "PM".into(),       total_min: 0.0, wait_min: 0.0 },
        ];
        let result = monthly_trend(&rows, 1, None);
        assert_eq!(result[0].month, "2024-01");
        assert_eq!(result[1].month, "2024-03");
    }
}
