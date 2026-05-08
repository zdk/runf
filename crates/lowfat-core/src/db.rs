use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// SQLite tracking database. Same schema as the bash version.
pub struct Db {
    conn: Connection,
}

/// A single tracked command execution.
pub struct TrackRecord {
    pub original_cmd: String,
    pub lowfat_cmd: String,
    pub raw: String,
    pub filtered: String,
    pub exec_time_ms: u64,
    pub project_path: String,
}

/// A single invocation record for the usage-history table.
pub struct InvocationRecord {
    pub command: String,
    pub subcommand: String,
    pub raw_tokens: u64,
    pub filtered_tokens: u64,
    /// A filter is registered for this command (regardless of subcommand scope).
    /// This is true for both native built-ins and external plugins.
    pub had_plugin: bool,
    /// Plugin applies to *this* subcommand — i.e. matches its declared list,
    /// or plugin declares no subcommand restriction.
    pub in_scope: bool,
    /// Filter actually shrank output (filtered_tokens < raw_tokens).
    pub reduced: bool,
    /// The handler is an external plugin from `~/.lowfat/plugins/` rather than
    /// a native built-in compiled into the binary. Used to label the source
    /// column in `lowfat history` so users can tell what they own vs what ships.
    pub is_external_plugin: bool,
    pub exit_code: i32,
}

/// One row of the plugin-candidates ranking.
#[derive(Debug)]
pub struct HistoryRow {
    pub command: String,
    pub subcommand: String,
    pub runs: u64,
    pub avg_raw_tokens: f64,
    pub total_raw_tokens: u64,
    pub savings_pct: f64,
    /// AVG(had_plugin): plugin registered for this command.
    pub registered_ratio: f64,
    /// AVG(in_scope): filter declared to cover this subcommand.
    pub in_scope_ratio: f64,
    /// AVG(reduced): filter actually shrank output on average.
    pub reduced_ratio: f64,
    /// AVG(is_external_plugin): handler came from `~/.lowfat/plugins/` rather
    /// than the binary's compiled-in filters. Drives the source column.
    pub external_ratio: f64,
    pub score: f64,
}

/// Exported invocation row (user-driven backup / analysis).
#[derive(Debug)]
pub struct InvocationExport {
    pub timestamp: String,
    pub command: String,
    pub subcommand: String,
    pub raw_tokens: u64,
    pub filtered_tokens: u64,
    pub had_plugin: bool,
    pub in_scope: bool,
    pub reduced: bool,
    pub is_external_plugin: bool,
    pub exit_code: i32,
}

/// Max rows retained in the `invocations` table. Oldest are evicted on insert.
const INVOCATIONS_CAP: i64 = 10_000;

/// Selective prune strategy for the `invocations` table.
#[derive(Debug, Clone)]
pub enum PruneFilter {
    /// Remove rows whose timestamp is older than N days.
    OlderThan(u32),
    /// Remove entire (command, subcommand) groups with fewer than N runs.
    BelowUsage(u64),
    /// Remove groups where every row already had a plugin — already filtered,
    /// so they aren't plugin candidates anymore.
    KeptByPlugin,
    /// Wipe the invocations table entirely.
    All,
}

/// Summary row from gain report.
#[derive(Debug)]
pub struct GainSummary {
    pub commands: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub saved_tokens: u64,
    pub savings_pct: f64,
}

/// Top command row from gain report.
#[derive(Debug)]
pub struct TopCommand {
    pub command: String,
    pub runs: u64,
    pub saved: i64,
    pub avg_pct: f64,
}

/// Session summary.
#[derive(Debug)]
pub struct SessionSummary {
    pub commands: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub saved_tokens: i64,
    pub savings_pct: f64,
    pub total_time_ms: u64,
}

impl Db {
    /// Open (or create) the tracking database.
    pub fn open(data_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let db_path = data_dir.join("history.db");
        let conn = Connection::open(&db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS commands (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                original_cmd TEXT NOT NULL,
                lowfat_cmd TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                saved_tokens INTEGER NOT NULL,
                savings_pct REAL NOT NULL,
                exec_time_ms INTEGER DEFAULT 0,
                project_path TEXT DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp);
            CREATE INDEX IF NOT EXISTS idx_project ON commands(project_path, timestamp);

            CREATE TABLE IF NOT EXISTS audit (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                plugin_name TEXT NOT NULL,
                runtime_type TEXT NOT NULL,
                command TEXT NOT NULL,
                action TEXT NOT NULL,
                checksum TEXT DEFAULT '',
                details TEXT DEFAULT ''
            );
            CREATE INDEX IF NOT EXISTS idx_audit_ts ON audit(timestamp);

            CREATE TABLE IF NOT EXISTS invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                command TEXT NOT NULL,
                subcommand TEXT NOT NULL DEFAULT '',
                raw_tokens INTEGER NOT NULL,
                filtered_tokens INTEGER NOT NULL,
                had_plugin INTEGER NOT NULL,
                exit_code INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_invocations_cmd ON invocations(command, subcommand);",
        )?;

        // Finer-grained status columns. Added after initial schema so existing
        // databases get migrated in place. `in_scope` distinguishes "plugin
        // declares this subcommand" from bare "plugin registered"; `reduced`
        // captures whether the filter actually shrank the output. Together
        // they split the misleading "has_plugin=yes, savings=0%" rows into
        // out-of-scope / noop / weak / good.
        let _ = conn.execute("ALTER TABLE invocations ADD COLUMN in_scope INTEGER", []);
        let _ = conn.execute("ALTER TABLE invocations ADD COLUMN reduced INTEGER", []);
        // Backfill legacy rows once. `reduced` is perfectly derivable from
        // stored token counts. `in_scope` can only be approximated — use
        // `had_plugin` as best-effort proxy (overestimates for subcommands
        // the plugin never covered, but only for rows predating migration).
        conn.execute(
            "UPDATE invocations SET reduced = CASE \
                WHEN filtered_tokens < raw_tokens THEN 1 ELSE 0 END \
             WHERE reduced IS NULL",
            [],
        )?;
        conn.execute(
            "UPDATE invocations SET in_scope = had_plugin WHERE in_scope IS NULL",
            [],
        )?;

        // `is_external_plugin` distinguishes external plugins (from
        // `~/.lowfat/plugins/`) from native built-ins compiled into the
        // binary. Without it, the source column conflates "lowfat ships a
        // filter for this" with "the user installed a plugin for this".
        let _ = conn.execute(
            "ALTER TABLE invocations ADD COLUMN is_external_plugin INTEGER",
            [],
        );
        // Legacy backfill: had_plugin=1 with command in the known native
        // builtin set is treated as built-in. Anything else with had_plugin=1
        // is treated as external. The native set is hardcoded to the names
        // shipped at this migration's authorship — accurate for historical
        // rows, since shipping new builtins requires a new binary anyway.
        conn.execute(
            "UPDATE invocations SET is_external_plugin = CASE \
                WHEN had_plugin = 1 AND command NOT IN ('git','docker','ls') THEN 1 \
                ELSE 0 END \
             WHERE is_external_plugin IS NULL",
            [],
        )?;
        Ok(Db { conn })
    }

    /// Record a command execution.
    pub fn track(&self, record: &TrackRecord) -> Result<()> {
        let in_tok = crate::tokens::estimate_tokens(&record.raw);
        let out_tok = crate::tokens::estimate_tokens(&record.filtered);
        let saved = in_tok as i64 - out_tok as i64;
        let pct = if in_tok > 0 {
            (saved as f64 / in_tok as f64) * 100.0
        } else {
            0.0
        };

        self.conn.execute(
            "INSERT INTO commands(timestamp, original_cmd, lowfat_cmd, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms, project_path)
             VALUES(datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                record.original_cmd,
                record.lowfat_cmd,
                in_tok as i64,
                out_tok as i64,
                saved,
                pct,
                record.exec_time_ms as i64,
                record.project_path,
            ],
        )?;
        Ok(())
    }

    /// Record an invocation for the usage-history ranking. Evicts oldest rows
    /// once the table grows past `INVOCATIONS_CAP`.
    pub fn record_invocation(&self, rec: &InvocationRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO invocations(timestamp, command, subcommand, raw_tokens, filtered_tokens, had_plugin, in_scope, reduced, is_external_plugin, exit_code)
             VALUES(datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                rec.command,
                rec.subcommand,
                rec.raw_tokens as i64,
                rec.filtered_tokens as i64,
                rec.had_plugin as i64,
                rec.in_scope as i64,
                rec.reduced as i64,
                rec.is_external_plugin as i64,
                rec.exit_code,
            ],
        )?;
        // id is monotonic (AUTOINCREMENT), so this trims only when over the cap.
        self.conn.execute(
            "DELETE FROM invocations WHERE id <= (SELECT MAX(id) - ?1 FROM invocations)",
            [INVOCATIONS_CAP],
        )?;
        Ok(())
    }

    /// Prune invocation rows matching `filter`. With `dry_run=true`, returns the
    /// count that would be removed without touching the table. Gain stats in the
    /// `commands` table are lifetime counters and are never pruned here.
    pub fn prune_invocations(&self, filter: &PruneFilter, dry_run: bool) -> Result<u64> {
        match filter {
            PruneFilter::All => {
                if dry_run {
                    let n: i64 = self.conn.query_row(
                        "SELECT COUNT(*) FROM invocations",
                        [],
                        |r| r.get(0),
                    )?;
                    Ok(n as u64)
                } else {
                    let n = self.conn.execute("DELETE FROM invocations", [])?;
                    Ok(n as u64)
                }
            }
            PruneFilter::OlderThan(days) => {
                let modifier = format!("-{days} days");
                if dry_run {
                    let n: i64 = self.conn.query_row(
                        "SELECT COUNT(*) FROM invocations WHERE timestamp < datetime('now', ?1)",
                        [&modifier],
                        |r| r.get(0),
                    )?;
                    Ok(n as u64)
                } else {
                    let n = self.conn.execute(
                        "DELETE FROM invocations WHERE timestamp < datetime('now', ?1)",
                        [&modifier],
                    )?;
                    Ok(n as u64)
                }
            }
            PruneFilter::BelowUsage(min) => {
                // Tuple-IN subquery: drop every row belonging to groups under the threshold.
                let count_sql = "SELECT COUNT(*) FROM invocations \
                    WHERE (command, subcommand) IN ( \
                        SELECT command, subcommand FROM invocations \
                        GROUP BY command, subcommand HAVING COUNT(*) < ?1)";
                let del_sql = "DELETE FROM invocations \
                    WHERE (command, subcommand) IN ( \
                        SELECT command, subcommand FROM invocations \
                        GROUP BY command, subcommand HAVING COUNT(*) < ?1)";
                let threshold = *min as i64;
                if dry_run {
                    let n: i64 = self.conn.query_row(count_sql, [threshold], |r| r.get(0))?;
                    Ok(n as u64)
                } else {
                    let n = self.conn.execute(del_sql, [threshold])?;
                    Ok(n as u64)
                }
            }
            PruneFilter::KeptByPlugin => {
                // MIN(had_plugin)=1 → every row in the group already had a plugin.
                let count_sql = "SELECT COUNT(*) FROM invocations \
                    WHERE (command, subcommand) IN ( \
                        SELECT command, subcommand FROM invocations \
                        GROUP BY command, subcommand HAVING MIN(had_plugin) = 1)";
                let del_sql = "DELETE FROM invocations \
                    WHERE (command, subcommand) IN ( \
                        SELECT command, subcommand FROM invocations \
                        GROUP BY command, subcommand HAVING MIN(had_plugin) = 1)";
                if dry_run {
                    let n: i64 = self.conn.query_row(count_sql, [], |r| r.get(0))?;
                    Ok(n as u64)
                } else {
                    let n = self.conn.execute(del_sql, [])?;
                    Ok(n as u64)
                }
            }
        }
    }

    /// Export all invocation rows (oldest first) for user-driven backup.
    pub fn export_invocations(&self) -> Result<Vec<InvocationExport>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, command, subcommand, raw_tokens, filtered_tokens,
                    had_plugin, COALESCE(in_scope,0), COALESCE(reduced,0),
                    COALESCE(is_external_plugin,0), exit_code
             FROM invocations ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(InvocationExport {
                timestamp: row.get(0)?,
                command: row.get(1)?,
                subcommand: row.get(2)?,
                raw_tokens: row.get::<_, i64>(3)? as u64,
                filtered_tokens: row.get::<_, i64>(4)? as u64,
                had_plugin: row.get::<_, i64>(5)? != 0,
                in_scope: row.get::<_, i64>(6)? != 0,
                reduced: row.get::<_, i64>(7)? != 0,
                is_external_plugin: row.get::<_, i64>(8)? != 0,
                exit_code: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Rank command+subcommand pairs as plugin candidates.
    ///
    /// Score = SUM(filtered_tokens) — bytes still flowing through to the LLM
    /// after filtering. High score = "lowfat hasn't tamed this row yet,"
    /// regardless of whether that's because there's no plugin or because the
    /// filter is weak. Note: the previous formula was effectively SUM(saved),
    /// which inverted the intent and put well-filtered rows at the top.
    ///
    /// `show_all=false` hides trivia (avg raw < 50 tokens or fewer than 2 runs)
    /// where no amount of plugin work could plausibly help.
    pub fn history_ranking(&self, limit: usize, show_all: bool) -> Result<Vec<HistoryRow>> {
        let extra_having = if show_all {
            ""
        } else {
            " AND AVG(raw_tokens) >= 50 AND COUNT(*) >= 2"
        };
        let sql = format!(
            "SELECT command, subcommand,
                    COUNT(*) AS runs,
                    AVG(raw_tokens) AS avg_raw,
                    SUM(raw_tokens) AS total_raw,
                    CASE WHEN SUM(raw_tokens) > 0
                         THEN 100.0 * (1.0 - 1.0 * SUM(filtered_tokens) / SUM(raw_tokens))
                         ELSE 0 END AS savings_pct,
                    AVG(had_plugin) AS registered_ratio,
                    AVG(COALESCE(in_scope,0)) AS in_scope_ratio,
                    AVG(COALESCE(reduced,0)) AS reduced_ratio,
                    AVG(COALESCE(is_external_plugin,0)) AS external_ratio,
                    SUM(filtered_tokens) AS score
             FROM invocations
             GROUP BY command, subcommand
             HAVING SUM(raw_tokens) > 0{extra_having}
             ORDER BY score DESC
             LIMIT ?1"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(HistoryRow {
                command: row.get(0)?,
                subcommand: row.get(1)?,
                runs: row.get::<_, i64>(2)? as u64,
                avg_raw_tokens: row.get(3)?,
                total_raw_tokens: row.get::<_, i64>(4)? as u64,
                savings_pct: row.get(5)?,
                registered_ratio: row.get(6)?,
                in_scope_ratio: row.get(7)?,
                reduced_ratio: row.get(8)?,
                external_ratio: row.get(9)?,
                score: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Lifetime gain summary.
    pub fn gain_summary(&self) -> Result<GainSummary> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(saved_tokens),0),
                    CASE WHEN SUM(input_tokens)>0
                      THEN ROUND(100.0*SUM(saved_tokens)/SUM(input_tokens),1) ELSE 0 END
             FROM commands",
        )?;
        let row = stmt.query_row([], |row| {
            Ok(GainSummary {
                commands: row.get::<_, i64>(0)? as u64,
                input_tokens: row.get::<_, i64>(1)? as u64,
                output_tokens: row.get::<_, i64>(2)? as u64,
                saved_tokens: row.get::<_, i64>(3)? as u64,
                savings_pct: row.get(4)?,
            })
        })?;
        Ok(row)
    }

    /// Top commands by tokens saved.
    pub fn top_commands(&self, limit: usize) -> Result<Vec<TopCommand>> {
        let mut stmt = self.conn.prepare(
            "SELECT original_cmd, COUNT(*), SUM(saved_tokens), ROUND(AVG(savings_pct),1)
             FROM commands GROUP BY original_cmd ORDER BY SUM(saved_tokens) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(TopCommand {
                command: row.get(0)?,
                runs: row.get::<_, i64>(1)? as u64,
                saved: row.get(2)?,
                avg_pct: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Session summary since a given timestamp.
    pub fn session_summary(&self, since: &str) -> Result<SessionSummary> {
        let mut stmt = self.conn.prepare(
            "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens),0),
                    COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(saved_tokens),0),
                    CASE WHEN SUM(input_tokens)>0
                      THEN ROUND(100.0*SUM(saved_tokens)/SUM(input_tokens),1) ELSE 0 END,
                    COALESCE(SUM(exec_time_ms),0)
             FROM commands WHERE timestamp >= ?1",
        )?;
        let row = stmt.query_row([since], |row| {
            Ok(SessionSummary {
                commands: row.get::<_, i64>(0)? as u64,
                input_tokens: row.get::<_, i64>(1)? as u64,
                output_tokens: row.get::<_, i64>(2)? as u64,
                saved_tokens: row.get(3)?,
                savings_pct: row.get(4)?,
                total_time_ms: row.get::<_, i64>(5)? as u64,
            })
        })?;
        Ok(row)
    }

    /// Record an audit event (plugin load, security check, etc.).
    pub fn audit(
        &self,
        plugin_name: &str,
        runtime_type: &str,
        command: &str,
        action: &str,
        checksum: &str,
        details: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit(timestamp, plugin_name, runtime_type, command, action, checksum, details)
             VALUES(datetime('now'), ?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![plugin_name, runtime_type, command, action, checksum, details],
        )?;
        Ok(())
    }

    /// Get recent audit entries.
    pub fn audit_log(&self, limit: usize) -> Result<Vec<AuditEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, plugin_name, runtime_type, command, action, checksum, details
             FROM audit ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(AuditEntry {
                timestamp: row.get(0)?,
                plugin_name: row.get(1)?,
                runtime_type: row.get(2)?,
                command: row.get(3)?,
                action: row.get(4)?,
                checksum: row.get(5)?,
                details: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

/// Audit log entry.
#[derive(Debug)]
pub struct AuditEntry {
    pub timestamp: String,
    pub plugin_name: String,
    pub runtime_type: String,
    pub command: String,
    pub action: String,
    pub checksum: String,
    pub details: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_create_and_track() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        let record = TrackRecord {
            original_cmd: "git status".to_string(),
            lowfat_cmd: "lowfat git status".to_string(),
            raw: "a".repeat(100), // 25 tokens
            filtered: "a".repeat(40), // 10 tokens
            exec_time_ms: 50,
            project_path: "/tmp/test".to_string(),
        };
        db.track(&record).unwrap();

        let summary = db.gain_summary().unwrap();
        assert_eq!(summary.commands, 1);
        assert_eq!(summary.input_tokens, 25);
        assert_eq!(summary.output_tokens, 10);
        assert_eq!(summary.saved_tokens, 15);
    }

    #[test]
    fn invocations_evict_past_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        // Insert cap + 5 rows, verify eviction keeps us at the cap.
        for i in 0..(super::INVOCATIONS_CAP + 5) {
            db.record_invocation(&InvocationRecord {
                command: "git".into(),
                subcommand: format!("s{i}"),
                raw_tokens: 100,
                filtered_tokens: 20,
                had_plugin: true,
                in_scope: true,
                reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            }).unwrap();
        }
        let count: i64 = db.conn
            .query_row("SELECT COUNT(*) FROM invocations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, super::INVOCATIONS_CAP);
    }

    #[test]
    fn history_ranking_orders_by_score() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        // "cargo build": 5 × 2000 × 0.05 savings = score 500 (big but barely filtered)
        for _ in 0..5 {
            db.record_invocation(&InvocationRecord {
                command: "cargo".into(), subcommand: "build".into(),
                raw_tokens: 2000, filtered_tokens: 1900,
                had_plugin: false, in_scope: false, reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            }).unwrap();
        }
        // "git status": 10 × 30 × 0.9 savings = score 270 (small and well-filtered)
        for _ in 0..10 {
            db.record_invocation(&InvocationRecord {
                command: "git".into(), subcommand: "status".into(),
                raw_tokens: 30, filtered_tokens: 3,
                had_plugin: true, in_scope: true, reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            }).unwrap();
        }

        // show_all=true: keep tiny git-status group so we can still assert both rows.
        let ranking = db.history_ranking(10, true).unwrap();
        assert_eq!(ranking.len(), 2);
        assert_eq!(ranking[0].command, "cargo");
        assert_eq!(ranking[0].subcommand, "build");
        assert_eq!(ranking[1].command, "git");

        // Default view (show_all=false) filters out avg_raw < 50 → git-status drops.
        let trimmed = db.history_ranking(10, false).unwrap();
        assert_eq!(trimmed.len(), 1);
        assert_eq!(trimmed[0].command, "cargo");
    }

    fn insert_dated_invocation(db: &Db, cmd: &str, sub: &str, had_plugin: bool, ts: &str) {
        db.conn.execute(
            "INSERT INTO invocations(timestamp, command, subcommand, raw_tokens, filtered_tokens, had_plugin, in_scope, reduced, exit_code)
             VALUES(?1, ?2, ?3, 100, 50, ?4, ?4, 1, 0)",
            rusqlite::params![ts, cmd, sub, had_plugin as i64],
        ).unwrap();
    }

    fn count_invocations(db: &Db) -> i64 {
        db.conn
            .query_row("SELECT COUNT(*) FROM invocations", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn prune_all_wipes_invocations_but_leaves_gain() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        for i in 0..3 {
            db.record_invocation(&InvocationRecord {
                command: "git".into(),
                subcommand: format!("s{i}"),
                raw_tokens: 100,
                filtered_tokens: 20,
                had_plugin: true,
                in_scope: true,
                reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            })
            .unwrap();
        }
        db.track(&TrackRecord {
            original_cmd: "git status".into(),
            lowfat_cmd: "lowfat git status".into(),
            raw: "a".repeat(100),
            filtered: "a".repeat(20),
            exec_time_ms: 5,
            project_path: "/tmp".into(),
        })
        .unwrap();

        let removed = db.prune_invocations(&PruneFilter::All, false).unwrap();
        assert_eq!(removed, 3);
        assert_eq!(count_invocations(&db), 0);
        // Gain totals (commands table) must be untouched.
        assert_eq!(db.gain_summary().unwrap().commands, 1);
    }

    #[test]
    fn prune_older_than_keeps_recent_drops_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        // One old row, one recent row.
        insert_dated_invocation(&db, "git", "log", false, "2020-01-01 00:00:00");
        insert_dated_invocation(&db, "git", "status", false, "2099-01-01 00:00:00");

        let removed = db
            .prune_invocations(&PruneFilter::OlderThan(30), false)
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(count_invocations(&db), 1);
    }

    #[test]
    fn prune_below_usage_drops_rare_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        // git status: 3 runs (keep); kubectl get: 1 run (drop at --below 2).
        for _ in 0..3 {
            db.record_invocation(&InvocationRecord {
                command: "git".into(),
                subcommand: "status".into(),
                raw_tokens: 50,
                filtered_tokens: 10,
                had_plugin: false,
                in_scope: false,
                reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            })
            .unwrap();
        }
        db.record_invocation(&InvocationRecord {
            command: "kubectl".into(),
            subcommand: "get".into(),
            raw_tokens: 4000,
            filtered_tokens: 4000,
            had_plugin: false,
            in_scope: false,
            reduced: false,
            is_external_plugin: false,
            exit_code: 0,
        })
        .unwrap();

        let preview = db
            .prune_invocations(&PruneFilter::BelowUsage(2), true)
            .unwrap();
        assert_eq!(preview, 1);
        assert_eq!(count_invocations(&db), 4); // dry-run didn't delete

        let removed = db
            .prune_invocations(&PruneFilter::BelowUsage(2), false)
            .unwrap();
        assert_eq!(removed, 1);
        assert_eq!(count_invocations(&db), 3);
    }

    #[test]
    fn prune_kept_by_plugin_drops_fully_covered_groups() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        // git status: all runs had a plugin → drop.
        for _ in 0..3 {
            db.record_invocation(&InvocationRecord {
                command: "git".into(),
                subcommand: "status".into(),
                raw_tokens: 50,
                filtered_tokens: 10,
                had_plugin: true,
                in_scope: true,
                reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            })
            .unwrap();
        }
        // kubectl get: no plugin coverage → keep.
        for _ in 0..2 {
            db.record_invocation(&InvocationRecord {
                command: "kubectl".into(),
                subcommand: "get".into(),
                raw_tokens: 4000,
                filtered_tokens: 4000,
                had_plugin: false,
                in_scope: false,
                reduced: false,
                is_external_plugin: false,
                exit_code: 0,
            })
            .unwrap();
        }

        let removed = db
            .prune_invocations(&PruneFilter::KeptByPlugin, false)
            .unwrap();
        assert_eq!(removed, 3);
        assert_eq!(count_invocations(&db), 2);
    }

    #[test]
    fn prune_dry_run_never_mutates() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();
        for _ in 0..5 {
            db.record_invocation(&InvocationRecord {
                command: "git".into(),
                subcommand: "log".into(),
                raw_tokens: 100,
                filtered_tokens: 50,
                had_plugin: false,
                in_scope: false,
                reduced: true,
                is_external_plugin: false,
                exit_code: 0,
            })
            .unwrap();
        }
        for filter in [
            PruneFilter::All,
            PruneFilter::BelowUsage(100),
            PruneFilter::KeptByPlugin,
            PruneFilter::OlderThan(0),
        ] {
            db.prune_invocations(&filter, true).unwrap();
            assert_eq!(count_invocations(&db), 5, "dry-run mutated with {filter:?}");
        }
    }

    #[test]
    fn top_commands() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Db::open(tmp.path()).unwrap();

        for _ in 0..3 {
            db.track(&TrackRecord {
                original_cmd: "git diff".to_string(),
                lowfat_cmd: "lowfat git diff".to_string(),
                raw: "a".repeat(100),
                filtered: "a".repeat(20),
                exec_time_ms: 10,
                project_path: "/tmp".to_string(),
            }).unwrap();
        }

        let top = db.top_commands(10).unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].command, "git diff");
        assert_eq!(top[0].runs, 3);
    }
}
