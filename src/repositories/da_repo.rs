use serde_json::Value;

use crate::{db::MssqlPool, errors::AppError};
use super::{mssql_util::*, wb_repo::build_shift_report};

/// DA report — identical structure to WB but area = 'DA'
pub struct DaRepo<'a> {
    pub pool:          &'a MssqlPool,
    pub view:          String,
    pub machine_table: String,
}

impl<'a> DaRepo<'a> {
    pub fn new(pool: &'a MssqlPool, view: impl Into<String>, machine_table: impl Into<String>) -> Self {
        Self { pool, view: view.into(), machine_table: machine_table.into() }
    }

    pub async fn packages(&self, date: &str) -> Result<Value, AppError> {
        let sql = format!(
            "SELECT DISTINCT [Package Type] AS package_type FROM {} \
             WHERE [id_operation] = 'DA' AND [Package Type] IS NOT NULL AND [Package Type] != '' \
               AND [datex] >= DATEADD(DAY, -7, @p1) AND [datex] < DATEADD(DAY, 1, @p1) \
             ORDER BY [Package Type]",
            self.view
        );
        let rows = exec(self.pool, &sql, &[date.to_string()]).await?;
        let pkgs: Vec<String> = rows.iter()
            .map(|r| str_val(r, "package_type"))
            .filter(|s| !s.is_empty())
            .collect();
        let mut opts: Vec<serde_json::Value> = vec![serde_json::json!({ "value": "__ALL__", "label": "— All Packages —" })];
        for p in &pkgs {
            opts.push(serde_json::json!({ "value": p, "label": p }));
        }
        Ok(serde_json::json!({ "options": opts, "packages": pkgs }))
    }

    pub async fn report(&self, date: &str, shift: &str, packages: &str) -> Result<Value, AppError> {
        let (shift_start, shift_end, time_range) = crate::repositories::wb_repo::WbRepo::shift_window(date, shift);

        let sql_ev = format!(
            "WITH key_mc AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine FROM {} \
               WHERE [id_operation] = 'DA' AND [flag_key] = 1 AND ISNULL([flag_delete],0) != 1 \
             ), shift_ev AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine, [job_type], [datex], [date_ack], \
                      [date_close], [des_job], ISNULL([Waiting_time],0) AS wait_min, [Package Type] AS package_type, \
                      NULLIF(RTRIM(LTRIM(ISNULL([by_perform],[by_ack]))),'') AS tech_name \
               FROM {} WHERE [id_operation]='DA' AND [datex]>=@p1 AND [datex]<@p2 \
                 AND [date_close] IS NOT NULL AND [date_close]>[datex] \
             ), pkg_scan AS ( \
               SELECT RTRIM(LTRIM([code_machine])) AS code_machine, [Package Type] AS package_type, \
                      ROW_NUMBER() OVER (PARTITION BY RTRIM(LTRIM([code_machine])) ORDER BY [datex] DESC) AS rn \
               FROM {} WHERE [id_operation]='DA' AND [Package Type] IS NOT NULL AND [Package Type]!='' \
                 AND [datex]>=DATEADD(DAY,-7,CAST(@p3 AS DATE)) AND [datex]<DATEADD(DAY,1,CAST(@p3 AS DATE)) \
             ), last_pkg AS (SELECT code_machine, package_type FROM pkg_scan WHERE rn=1) \
             SELECT k.code_machine, se.job_type, se.datex, se.date_ack, se.date_close, se.des_job, \
                    se.wait_min, se.tech_name, COALESCE(se.package_type,lp.package_type) AS package_type, \
                    FORMAT(se.datex,'HH:mm') AS t_start_fmt, FORMAT(se.date_close,'HH:mm') AS t_end_fmt \
             FROM key_mc k LEFT JOIN shift_ev se ON k.code_machine=se.code_machine \
             LEFT JOIN last_pkg lp ON k.code_machine=lp.code_machine ORDER BY k.code_machine, se.datex",
            self.machine_table, self.view, self.view
        );
        let sql_open = "SELECT RTRIM(LTRIM(code_machine)) AS code_machine, job_type, des_job, \
            FORMAT(datex,'HH:mm') AS t_start, DATEDIFF(MINUTE,datex,GETDATE()) AS dur_min \
            FROM dbo.job_list WHERE id_operation='DA' AND date_close IS NULL \
            AND code_machine!='' AND LEN(code_machine)>3".to_string();

        let ev_params = vec![shift_start, shift_end, date.to_string()];
        let (ev_rows, open_rows) = tokio::try_join!(
            exec(self.pool, &sql_ev, &ev_params),
            exec(self.pool, &sql_open, &[]),
        )?;

        Ok(build_shift_report(&ev_rows, &open_rows, packages, shift, &time_range))
    }
}
