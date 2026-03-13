use std::fmt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::Connection;

pub struct StatsDb {
    conn: Connection,
}

impl StatsDb {
    pub fn open(base_path: Option<&Path>) -> Result<Self> {
        let base = match base_path {
            Some(p) => p.to_path_buf(),
            None => {
                let home = std::env::var("CODE_INDEX_PATH").unwrap_or_else(|_| {
                    let h = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                    format!("{h}/.code-index")
                });
                std::path::PathBuf::from(home)
            }
        };
        std::fs::create_dir_all(&base).ok();
        let db_path = base.join("_stats.db");
        let conn = Connection::open(&db_path)
            .with_context(|| format!("Failed to open stats DB at {}", db_path.display()))?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS query_log (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               tool_name TEXT NOT NULL,
               repo TEXT,
               timestamp TEXT NOT NULL,
               duration_ms INTEGER NOT NULL,
               response_bytes INTEGER NOT NULL,
               estimated_tokens INTEGER NOT NULL,
               full_file_bytes INTEGER NOT NULL DEFAULT 0
             );",
        )?;

        // Migrate existing DBs that lack the column.
        let has_col: bool = conn
            .prepare("SELECT full_file_bytes FROM query_log LIMIT 0")
            .is_ok();
        if !has_col {
            conn.execute_batch(
                "ALTER TABLE query_log ADD COLUMN full_file_bytes INTEGER NOT NULL DEFAULT 0;",
            )?;
        }

        Ok(Self { conn })
    }

    /// Record a tool call. Fire-and-forget — logs errors but never panics.
    pub fn record(
        &self,
        tool_name: &str,
        repo: Option<&str>,
        duration_ms: u64,
        response_bytes: usize,
        full_file_bytes: usize,
    ) {
        let estimated_tokens = response_bytes / 4;
        let timestamp = iso_now();
        if let Err(e) = self.conn.execute(
            "INSERT INTO query_log (tool_name, repo, timestamp, duration_ms, response_bytes, estimated_tokens, full_file_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                tool_name,
                repo,
                timestamp,
                duration_ms,
                response_bytes as i64,
                estimated_tokens as i64,
                full_file_bytes as i64,
            ],
        ) {
            tracing::warn!("Failed to record stats: {e}");
        }
    }

    /// Clear all recorded stats.
    pub fn reset(&self) -> Result<()> {
        self.conn.execute("DELETE FROM query_log", [])?;
        Ok(())
    }

    /// Aggregate usage summary with optional filters.
    pub fn summary(&self, repo: Option<&str>, since: Option<&str>) -> Result<Option<StatsSummary>> {
        let (where_clause, params) = build_filter(repo, since, "");

        let total_queries: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM query_log{where_clause}"),
            rusqlite::params_from_iter(&params),
            |r| r.get(0),
        )?;

        if total_queries == 0 {
            return Ok(None);
        }

        let (total_tokens, avg_duration_ms): (i64, f64) = self.conn.query_row(
            &format!(
                "SELECT COALESCE(SUM(estimated_tokens),0),
                        COALESCE(AVG(duration_ms),0)
                 FROM query_log{where_clause}"
            ),
            rusqlite::params_from_iter(&params),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let (first_query, last_query): (String, String) = self.conn.query_row(
            &format!("SELECT MIN(timestamp), MAX(timestamp) FROM query_log{where_clause}"),
            rusqlite::params_from_iter(&params),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let mut stmt = self.conn.prepare(&format!(
            "SELECT tool_name,
                    COUNT(*) as cnt,
                    SUM(estimated_tokens) as tok,
                    AVG(duration_ms) as avg_ms
             FROM query_log{where_clause}
             GROUP BY tool_name
             ORDER BY cnt DESC"
        ))?;

        let tool_stats: Vec<ToolStat> = stmt
            .query_map(rusqlite::params_from_iter(&params), |r| {
                Ok(ToolStat {
                    name: r.get(0)?,
                    count: r.get(1)?,
                    tokens: r.get(2)?,
                    avg_ms: r.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(Some(StatsSummary {
            total_queries: total_queries as u64,
            total_tokens: total_tokens as u64,
            avg_duration_ms,
            first_query,
            last_query,
            by_tool: tool_stats,
        }))
    }

    /// Token savings summary for `repomap gain` with optional filters.
    pub fn gain(&self, repo: Option<&str>, since: Option<&str>) -> Result<Option<GainSummary>> {
        let (where_clause, params) = build_filter(repo, since, " AND full_file_bytes > 0");

        let total_queries: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM query_log{where_clause}"),
            rusqlite::params_from_iter(&params),
            |r| r.get(0),
        )?;

        if total_queries == 0 {
            return Ok(None);
        }

        let (total_output_tokens, total_full_tokens, avg_duration_ms): (i64, i64, f64) =
            self.conn.query_row(
                &format!(
                    "SELECT COALESCE(SUM(estimated_tokens),0),
                            COALESCE(SUM(full_file_bytes / 4),0),
                            COALESCE(AVG(duration_ms),0)
                     FROM query_log{where_clause}"
                ),
                rusqlite::params_from_iter(&params),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;

        let total_saved = total_full_tokens - total_output_tokens;

        let mut stmt = self.conn.prepare(&format!(
            "SELECT tool_name,
                    COUNT(*) as cnt,
                    SUM(estimated_tokens) as out_tok,
                    SUM(full_file_bytes / 4) as full_tok,
                    AVG(duration_ms) as avg_ms
             FROM query_log{where_clause}
             GROUP BY tool_name
             ORDER BY (SUM(full_file_bytes / 4) - SUM(estimated_tokens)) DESC"
        ))?;

        let by_tool: Vec<GainToolStat> = stmt
            .query_map(rusqlite::params_from_iter(&params), |r| {
                let out_tok: i64 = r.get(2)?;
                let full_tok: i64 = r.get(3)?;
                Ok(GainToolStat {
                    name: r.get(0)?,
                    count: r.get(1)?,
                    output_tokens: out_tok,
                    full_tokens: full_tok,
                    saved: full_tok - out_tok,
                    avg_ms: r.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        let pct = if total_full_tokens > 0 {
            (total_saved as f64 / total_full_tokens as f64) * 100.0
        } else {
            0.0
        };

        Ok(Some(GainSummary {
            total_queries: total_queries as u64,
            total_output_tokens: total_output_tokens as u64,
            total_full_tokens: total_full_tokens as u64,
            total_saved: total_saved as u64,
            pct_saved: pct,
            avg_duration_ms,
            by_tool,
        }))
    }
}

/// Build a WHERE clause from optional repo and since filters.
fn build_filter(repo: Option<&str>, since: Option<&str>, extra: &str) -> (String, Vec<String>) {
    let mut conditions = Vec::new();
    let mut params = Vec::new();

    if let Some(r) = repo {
        conditions.push("repo = ?".to_string());
        params.push(r.to_string());
    }
    if let Some(s) = since {
        conditions.push("timestamp >= ?".to_string());
        params.push(s.to_string());
    }
    if !extra.is_empty() {
        conditions.push(extra.trim_start_matches(" AND ").to_string());
    }

    let clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    (clause, params)
}

/// Compute an ISO 8601 timestamp N days ago from now.
pub fn days_ago_iso(days: u32) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(days as u64 * 86400);

    let days_epoch = secs / 86400;
    let (year, month, day) = epoch_days_to_date(days_epoch as i64);
    format!("{year:04}-{month:02}-{day:02}T00:00:00Z")
}

pub struct GainSummary {
    pub total_queries: u64,
    pub total_output_tokens: u64,
    pub total_full_tokens: u64,
    pub total_saved: u64,
    pub pct_saved: f64,
    pub avg_duration_ms: f64,
    pub by_tool: Vec<GainToolStat>,
}

pub struct GainToolStat {
    pub name: String,
    pub count: i64,
    pub output_tokens: i64,
    pub full_tokens: i64,
    pub saved: i64,
    pub avg_ms: f64,
}

impl fmt::Display for GainSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "repomap Token Savings")?;
        writeln!(f, "════════════════════════════════════════════════════════════")?;
        writeln!(f)?;
        writeln!(f, "Total queries:     {}", fmt_num(self.total_queries))?;
        writeln!(f, "Full file tokens:  {}", fmt_num(self.total_full_tokens))?;
        writeln!(f, "Tokens served:     {}", fmt_num(self.total_output_tokens))?;
        writeln!(
            f,
            "Tokens saved:      {} ({:.1}%)",
            fmt_num(self.total_saved),
            self.pct_saved
        )?;
        writeln!(f, "Avg response time: {:.0}ms", self.avg_duration_ms)?;

        // Efficiency bar
        let bar_width: usize = 24;
        let filled = ((self.pct_saved / 100.0) * bar_width as f64).round() as usize;
        let empty = bar_width.saturating_sub(filled);
        writeln!(
            f,
            "Efficiency:        {}{} {:.1}%",
            "█".repeat(filled),
            "░".repeat(empty),
            self.pct_saved
        )?;

        writeln!(f)?;
        writeln!(f, "By Tool")?;
        writeln!(
            f,
            "───────────────────────────────────────────────────────────────────────"
        )?;
        writeln!(
            f,
            "  {:<22} {:>5}  {:>10}  {:>6}  {:>6}  {}",
            "Tool", "Count", "Saved", "Avg%", "Time", "Impact"
        )?;
        writeln!(
            f,
            "───────────────────────────────────────────────────────────────────────"
        )?;

        let max_saved = self.by_tool.iter().map(|t| t.saved).max().unwrap_or(1).max(1);

        for ts in &self.by_tool {
            let pct = if ts.full_tokens > 0 {
                (ts.saved as f64 / ts.full_tokens as f64) * 100.0
            } else {
                0.0
            };
            let bar_len = ((ts.saved as f64 / max_saved as f64) * 10.0).round() as usize;
            writeln!(
                f,
                "  {:<22} {:>5}  {:>10}  {:>5.1}%  {:>4.0}ms  {}",
                ts.name,
                fmt_num(ts.count as u64),
                fmt_num(ts.saved as u64),
                pct,
                ts.avg_ms,
                "█".repeat(bar_len),
            )?;
        }
        writeln!(
            f,
            "───────────────────────────────────────────────────────────────────────"
        )?;
        Ok(())
    }
}

pub struct StatsSummary {
    pub total_queries: u64,
    pub total_tokens: u64,
    pub avg_duration_ms: f64,
    pub first_query: String,
    pub last_query: String,
    pub by_tool: Vec<ToolStat>,
}

pub struct ToolStat {
    pub name: String,
    pub count: i64,
    pub tokens: i64,
    pub avg_ms: f64,
}

impl fmt::Display for StatsSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "repomap usage stats")?;
        writeln!(f, "═══════════════════════════════════════")?;
        writeln!(f, "Total queries:     {}", fmt_num(self.total_queries))?;
        writeln!(f, "Tokens served:     {}", fmt_num(self.total_tokens))?;
        writeln!(f, "Avg response time: {:.0}ms", self.avg_duration_ms)?;
        // Show just the date portion of the first query
        let since = self.first_query.split('T').next().unwrap_or(&self.first_query);
        let last = self.last_query.split('T').next().unwrap_or(&self.last_query);
        writeln!(f, "Active since:      {since}")?;
        writeln!(f, "Last query:        {last}")?;
        writeln!(f)?;
        writeln!(f, "By tool:")?;
        for ts in &self.by_tool {
            writeln!(
                f,
                "  {:<22} {:>5} calls   {:>8} tokens   {:.0}ms avg",
                ts.name,
                fmt_num(ts.count as u64),
                fmt_num(ts.tokens as u64),
                ts.avg_ms,
            )?;
        }
        Ok(())
    }
}

fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Convert epoch seconds to ISO 8601 date-time (UTC, no chrono dependency)
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    // Days since 1970-01-01
    let (year, month, day) = epoch_days_to_date(days as i64);

    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn epoch_days_to_date(mut days: i64) -> (i64, u32, u32) {
    // Civil from days algorithm (Howard Hinnant)
    days += 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let doe = (days - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
