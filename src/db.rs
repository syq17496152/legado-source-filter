use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, Result as SqlResult, params};

/// (status, reason, tested_at, retry_count, evidence) from `test_results`.
type RowFull = (String, Option<String>, i64, u32, Option<String>);

/// Thread-safe wrapper around a single SQLite connection.
///
/// All access is serialized through an internal mutex, so one shared
/// instance can be used across concurrent tasks (unlike opening a new
/// connection per task, which risks `SQLITE_BUSY` under concurrent writes).
pub struct TestCache {
    conn: Mutex<Connection>,
}

impl TestCache {
    pub fn new(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS test_results (
                book_source_url TEXT NOT NULL,
                book_source_name TEXT NOT NULL,
                status TEXT NOT NULL,
                reason TEXT,
                tested_at INTEGER NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (book_source_url, book_source_name)
            );
            CREATE INDEX IF NOT EXISTS idx_results_status ON test_results(status);
            CREATE TABLE IF NOT EXISTS pipeline_data (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS test_history (
                book_source_url TEXT NOT NULL,
                book_source_name TEXT NOT NULL,
                tested_at INTEGER NOT NULL,
                status TEXT NOT NULL,
                reason TEXT,
                evidence TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_history_source ON test_history(book_source_url, book_source_name);",
        )?;
        // Schema v2 migration: `evidence` (JSON detail of the last attempt) on
        // test_results. Older caches lack the column; add it in place so
        // existing databases keep working.
        ensure_column(
            &conn,
            "test_results",
            "evidence",
            "TEXT",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn check(&self, url: &str, name: &str) -> SqlResult<Option<(String, Option<String>)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT status, reason FROM test_results WHERE book_source_url = ?1 AND book_source_name = ?2"
        )?;
        let mut rows = stmt.query(params![url, name])?;
        if let Some(row) = rows.next()? {
            let status: String = row.get(0)?;
            let reason: Option<String> = row.get(1)?;
            Ok(Some((status, reason)))
        } else {
            Ok(None)
        }
    }

    /// Full row lookup: status, reason, last-tested timestamp, retry count and
    /// the JSON evidence of the last attempt.
    pub fn check_full(
        &self,
        url: &str,
        name: &str,
    ) -> SqlResult<Option<RowFull>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT status, reason, tested_at, retry_count, evidence FROM test_results
             WHERE book_source_url = ?1 AND book_source_name = ?2",
        )?;
        let mut rows = stmt.query(params![url, name])?;
        if let Some(row) = rows.next()? {
            Ok(Some((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            )))
        } else {
            Ok(None)
        }
    }

    /// Returns per-status counts from the test cache.
    pub fn summary(&self) -> SqlResult<Vec<(String, usize)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM test_results GROUP BY status")?;
        let rows = stmt.query_map([], |row| {
            let status: String = row.get(0)?;
            let count: usize = row.get(1)?;
            Ok((status, count))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Save a key-value metadata entry (e.g. "eligible", "report").
    pub fn save_meta(&self, key: &str, value: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO pipeline_data (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Load a metadata entry by key.
    pub fn load_meta(&self, key: &str) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT value FROM pipeline_data WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn save(
        &self,
        url: &str,
        name: &str,
        status: &str,
        reason: Option<&str>,
        retry_count: u32,
    ) -> SqlResult<()> {
        self.save_with_evidence(url, name, status, reason, retry_count, None)
    }

    /// Save a test verdict together with JSON evidence about the attempt
    /// (searched keywords, verified chain count, content length, ...).
    pub fn save_with_evidence(
        &self,
        url: &str,
        name: &str,
        status: &str,
        reason: Option<&str>,
        retry_count: u32,
        evidence: Option<&str>,
    ) -> SqlResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO test_results (book_source_url, book_source_name, status, reason, tested_at, retry_count, evidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![url, name, status, reason, now, retry_count, evidence],
        )?;
        Ok(())
    }

    /// Append one row to the append-only history (one entry per terminal
    /// verdict). Used for stability / regression analysis across runs.
    pub fn history_append(
        &self,
        url: &str,
        name: &str,
        status: &str,
        reason: Option<&str>,
        evidence: Option<&str>,
    ) -> SqlResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO test_history (book_source_url, book_source_name, tested_at, status, reason, evidence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![url, name, now, status, reason, evidence],
        )?;
        Ok(())
    }

    /// Load the JSON evidence of a source's last attempt.
    pub fn evidence_of(&self, url: &str, name: &str) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT evidence FROM test_results WHERE book_source_url = ?1 AND book_source_name = ?2",
        )?;
        let mut rows = stmt.query(params![url, name])?;
        if let Some(row) = rows.next()? {
            row.get(0)
        } else {
            Ok(None)
        }
    }
}

/// Add a column to a table if it doesn't exist yet (SQLite has no
/// `ADD COLUMN IF NOT EXISTS`). Best-effort schema migration.
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> SqlResult<()> {
    let sql = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    if cols.iter().any(|c| c == column) {
        return Ok(());
    }
    conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lsf-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
        path
    }

    #[test]
    fn test_save_check_summary_roundtrip() {
        let path = temp_db("roundtrip.db");
        let cache = TestCache::new(&path).unwrap();

        assert_eq!(cache.check("https://a.com", "A").unwrap(), None);

        cache.save("https://a.com", "A", "passed", None, 1).unwrap();
        cache
            .save("https://b.com", "B", "failed", Some("network_error"), 1)
            .unwrap();
        cache
            .save("https://c.com", "C", "dead_domain", Some("no_results"), 0)
            .unwrap();

        let (status, reason) = cache.check("https://b.com", "B").unwrap().unwrap();
        assert_eq!(status, "failed");
        assert_eq!(reason.as_deref(), Some("network_error"));

        let summary = cache.summary().unwrap();
        let map: std::collections::HashMap<String, usize> = summary.into_iter().collect();
        assert_eq!(map.get("passed"), Some(&1));
        assert_eq!(map.get("failed"), Some(&1));
        assert_eq!(map.get("dead_domain"), Some(&1));
    }

    #[test]
    fn test_meta_roundtrip() {
        let path = temp_db("meta.db");
        let cache = TestCache::new(&path).unwrap();
        assert_eq!(cache.load_meta("report").unwrap(), None);

        cache.save_meta("report", r#"{"summary":{}}"#).unwrap();
        assert_eq!(
            cache.load_meta("report").unwrap().as_deref(),
            Some(r#"{"summary":{}}"#)
        );
    }

    #[test]
    fn test_update_existing_row() {
        let path = temp_db("update.db");
        let cache = TestCache::new(&path).unwrap();

        cache
            .save("https://a.com", "A", "failed", Some("no_results"), 1)
            .unwrap();
        cache.save("https://a.com", "A", "passed", None, 1).unwrap();

        let (status, reason) = cache.check("https://a.com", "A").unwrap().unwrap();
        assert_eq!(status, "passed");
        assert_eq!(reason, None);
        let summary = cache.summary().unwrap();
        assert_eq!(summary.len(), 1, "single row, overwritten");
    }

    #[test]
    fn test_evidence_and_history() {
        let path = temp_db("evidence.db");
        let cache = TestCache::new(&path).unwrap();

        cache
            .save_with_evidence(
                "https://a.com",
                "A",
                "passed",
                None,
                1,
                Some(r#"{"chains_ok":1,"keyword":"重生"}"#),
            )
            .unwrap();
        let (st, reason, tested_at, retry, evidence) =
            cache.check_full("https://a.com", "A").unwrap().unwrap();
        assert_eq!(st, "passed");
        assert_eq!(reason, None);
        assert!(tested_at > 0);
        assert_eq!(retry, 1);
        assert_eq!(
            evidence.as_deref(),
            Some(r#"{"chains_ok":1,"keyword":"重生"}"#)
        );
        assert_eq!(
            cache
                .evidence_of("https://a.com", "A")
                .unwrap()
                .as_deref(),
            Some(r#"{"chains_ok":1,"keyword":"重生"}"#)
        );

        // Plain save() keeps old rows' evidence after an overwrite? No — it
        // clears it, which is correct for a fresh attempt without evidence.
        cache
            .save("https://a.com", "A", "failed", Some("no_results"), 2)
            .unwrap();
        assert_eq!(
            cache.evidence_of("https://a.com", "A").unwrap(),
            None,
            "evidence cleared when saving without evidence"
        );

        cache
            .history_append("https://a.com", "A", "failed", Some("no_results"), None)
            .unwrap();
        cache
            .history_append("https://a.com", "A", "passed", None, Some("{}"))
            .unwrap();
        let conn = cache.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM test_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2, "history is append-only");
    }

    #[test]
    fn test_migrates_legacy_schema() {
        // Create a DB with the OLD schema (no evidence column), then open it
        // with TestCache — the column must be added in place.
        let path = temp_db("legacy.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE test_results (
                    book_source_url TEXT NOT NULL,
                    book_source_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    reason TEXT,
                    tested_at INTEGER NOT NULL,
                    retry_count INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (book_source_url, book_source_name)
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO test_results VALUES (?1,?2,?3,?4,?5,?6)",
                params!["https://old.com", "Old", "passed", None::<String>, 1000i64, 1u32],
            )
            .unwrap();
        }
        let cache = TestCache::new(&path).unwrap();
        let (st, _, _, retry, evidence) =
            cache.check_full("https://old.com", "Old").unwrap().unwrap();
        assert_eq!(st, "passed");
        assert_eq!(retry, 1);
        assert_eq!(evidence, None, "legacy row has no evidence yet");

        // After migration, evidence writes must work.
        cache
            .save_with_evidence("https://old.com", "Old", "passed", None, 2, Some("{}"))
            .unwrap();
        let row = cache.check_full("https://old.com", "Old").unwrap().unwrap();
        assert_eq!(row.3, 2);
        assert_eq!(row.4.as_deref(), Some("{}"));
    }
}
