use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, Result as SqlResult};

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
            );"
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

    /// Returns per-status counts from the test cache.
    pub fn summary(&self) -> SqlResult<Vec<(String, usize)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT status, COUNT(*) FROM test_results GROUP BY status"
        )?;
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
        let mut stmt = conn.prepare(
            "SELECT value FROM pipeline_data WHERE key = ?1"
        )?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn save(&self, url: &str, name: &str, status: &str, reason: Option<&str>, retry_count: u32) -> SqlResult<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO test_results (book_source_url, book_source_name, status, reason, tested_at, retry_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![url, name, status, reason, now, retry_count],
        )?;
        Ok(())
    }
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
        cache.save("https://b.com", "B", "failed", Some("network_error"), 1).unwrap();
        cache.save("https://c.com", "C", "dead_domain", Some("no_results"), 0).unwrap();

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

        cache.save("https://a.com", "A", "failed", Some("no_results"), 1).unwrap();
        cache.save("https://a.com", "A", "passed", None, 1).unwrap();

        let (status, reason) = cache.check("https://a.com", "A").unwrap().unwrap();
        assert_eq!(status, "passed");
        assert_eq!(reason, None);
        let summary = cache.summary().unwrap();
        assert_eq!(summary.len(), 1, "single row, overwritten");
    }
}
