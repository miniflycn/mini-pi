use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::PathBuf;

pub struct Store {
    conn: Mutex<Connection>,
    sessions_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct ThreadMeta {
    pub id: String,
    pub title: String,
    pub preview: String,
    pub session_file: Option<String>,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub pinned: bool,
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct PaginatedThreads {
    pub threads: Vec<ThreadMeta>,
    pub page: usize,
    pub per_page: usize,
    pub total: usize,
}

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_init",
        "
        CREATE TABLE threads (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL DEFAULT '',
            preview       TEXT NOT NULL DEFAULT '',
            session_file  TEXT,
            model         TEXT,
            pinned        INTEGER NOT NULL DEFAULT 0,
            created_at    DATETIME NOT NULL DEFAULT (datetime('now')),
            updated_at    DATETIME NOT NULL DEFAULT (datetime('now'))
        );
        ",
    ),
    (
        "002_workspaces",
        "
        CREATE TABLE workspaces (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            path        TEXT NOT NULL UNIQUE,
            created_at  DATETIME NOT NULL DEFAULT (datetime('now')),
            updated_at  DATETIME NOT NULL DEFAULT (datetime('now'))
        );
        ",
    ),
    (
        "003_user_settings",
        "
        CREATE TABLE user_settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        ",
    ),
    (
        "004_thinking_level",
        "
        ALTER TABLE threads ADD COLUMN thinking_level TEXT;
        ",
    ),
    (
        "005_thread_metadata",
        "
        ALTER TABLE threads ADD COLUMN metadata TEXT;
        ",
    ),
];

const THREAD_SELECT_COLUMNS: &str = "id, title, preview, session_file, model, thinking_level, pinned, metadata, \
     created_at, updated_at";

fn row_to_thread(row: &Row) -> rusqlite::Result<ThreadMeta> {
    let metadata_str: Option<String> = row.get(7)?;
    let metadata = metadata_str.and_then(|s| serde_json::from_str(&s).ok());
    Ok(ThreadMeta {
        id: row.get(0)?,
        title: row.get(1)?,
        preview: row.get(2)?,
        session_file: row.get(3)?,
        model: row.get(4)?,
        thinking_level: row.get(5)?,
        pinned: row.get::<_, i32>(6)? != 0,
        metadata,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn row_to_workspace(row: &Row) -> rusqlite::Result<WorkspaceMeta> {
    Ok(WorkspaceMeta {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
    })
}

impl Store {
    #[cfg(test)]
    pub fn new_test(dir: &std::path::Path) -> Result<Self, StoreError> {
        std::fs::create_dir_all(dir).map_err(StoreError::Io)?;
        std::fs::create_dir_all(dir.join("sessions")).map_err(StoreError::Io)?;

        let db_path = dir.join("mini-pi.db");
        let conn = Connection::open(&db_path).map_err(StoreError::Rusqlite)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(StoreError::Rusqlite)?;

        let store = Self {
            conn: Mutex::new(conn),
            sessions_dir: dir.join("sessions"),
        };
        store.run_migrations()?;
        Ok(store)
    }

    pub fn open() -> Result<Self, StoreError> {
        let dir = dirs::home_dir()
            .ok_or(StoreError::HomeDir)?
            .join(".mini-pi");
        std::fs::create_dir_all(&dir).map_err(StoreError::Io)?;
        std::fs::create_dir_all(dir.join("sessions")).map_err(StoreError::Io)?;

        let db_path = dir.join("mini-pi.db");
        let conn = Connection::open(&db_path).map_err(StoreError::Rusqlite)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")
            .map_err(StoreError::Rusqlite)?;

        let store = Self {
            conn: Mutex::new(conn),
            sessions_dir: dir.join("sessions"),
        };
        store.run_migrations()?;
        Ok(store)
    }

    fn run_migrations(&self) -> Result<(), StoreError> {
        // Migrations are run with WAL mode which uses internal locking, but
        // we take the mutex around the whole batch to keep semantics simple.
        let conn = self.conn.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS _migrations (
                    name  TEXT PRIMARY KEY,
                    applied_at DATETIME NOT NULL DEFAULT (datetime('now'))
                );",
        )
        .map_err(StoreError::Rusqlite)?;

        for &(name, sql) in MIGRATIONS {
            let applied: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM _migrations WHERE name = ?1",
                    params![name],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false);

            if !applied {
                // Each migration is its own implicit transaction. ALTER TABLE
                // cannot run inside a multi-statement transaction in SQLite, so
                // `execute_batch` is the only option here.
                conn.execute_batch(sql).map_err(StoreError::Rusqlite)?;
                conn.execute("INSERT INTO _migrations (name) VALUES (?1)", params![name])
                    .map_err(StoreError::Rusqlite)?;
            }
        }

        Ok(())
    }

    pub fn sessions_dir(&self) -> &PathBuf {
        &self.sessions_dir
    }

    pub fn set_user_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO user_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(StoreError::Rusqlite)?;
        Ok(())
    }

    pub fn get_user_setting(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT value FROM user_settings WHERE key = ?1")
            .map_err(StoreError::Rusqlite)?;
        stmt.query_row(params![key], |row| row.get::<_, String>(0))
            .optional()
            .map_err(StoreError::Rusqlite)
    }

    pub fn delete_user_setting(&self, key: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM user_settings WHERE key = ?1", params![key])
            .map_err(StoreError::Rusqlite)?;
        Ok(())
    }

    /// Fetch a thread by id using an existing connection. Returns
    /// `Ok(None)` when the row doesn't exist (so callers can compose this
    /// inside a transaction without re-locking the mutex).
    fn get_thread_by_conn(conn: &Connection, id: &str) -> Result<Option<ThreadMeta>, StoreError> {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {THREAD_SELECT_COLUMNS} FROM threads WHERE id = ?1"
            ))
            .map_err(StoreError::Rusqlite)?;
        stmt.query_row(params![id], row_to_thread)
            .optional()
            .map_err(StoreError::Rusqlite)
    }

    fn get_workspace_by_conn(
        conn: &Connection,
        id: &str,
    ) -> Result<Option<WorkspaceMeta>, StoreError> {
        let mut stmt = conn
            .prepare("SELECT id, name, path, created_at, updated_at FROM workspaces WHERE id = ?1")
            .map_err(StoreError::Rusqlite)?;
        stmt.query_row(params![id], row_to_workspace)
            .optional()
            .map_err(StoreError::Rusqlite)
    }

    pub fn create_thread(&self, title: &str, preview: &str) -> Result<ThreadMeta, StoreError> {
        let mut conn = self.conn.lock();
        let id = nanoid::nanoid!();
        let tx = conn.transaction().map_err(StoreError::Rusqlite)?;
        tx.execute(
            "INSERT INTO threads (id, title, preview, thinking_level, metadata) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, title, preview, Option::<&str>::None, Option::<String>::None],
        )
        .map_err(StoreError::Rusqlite)?;
        let thread = Self::get_thread_by_conn(&tx, &id)?.ok_or(StoreError::MissingRow(format!(
            "thread {} after create",
            id
        )))?;
        tx.commit().map_err(StoreError::Rusqlite)?;
        Ok(thread)
    }

    pub fn list_threads(&self) -> Result<Vec<ThreadMeta>, StoreError> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {THREAD_SELECT_COLUMNS} FROM threads ORDER BY pinned DESC, updated_at DESC"
            ))
            .map_err(StoreError::Rusqlite)?;
        let rows = stmt
            .query_map([], row_to_thread)
            .map_err(StoreError::Rusqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Rusqlite)
    }

    pub fn list_threads_paginated(
        &self,
        page: usize,
        per_page: usize,
    ) -> Result<PaginatedThreads, StoreError> {
        let page = page.max(1);
        let per_page = per_page.max(1);
        let offset = (page - 1) * per_page;

        let conn = self.conn.lock();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM threads", [], |row| row.get(0))
            .map_err(StoreError::Rusqlite)?;

        let mut stmt = conn
            .prepare(&format!(
                "SELECT {THREAD_SELECT_COLUMNS} FROM threads \
                 ORDER BY pinned DESC, updated_at DESC \
                 LIMIT ?1 OFFSET ?2"
            ))
            .map_err(StoreError::Rusqlite)?;
        let rows = stmt
            .query_map(params![per_page as i64, offset as i64], row_to_thread)
            .map_err(StoreError::Rusqlite)?;
        let threads = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Rusqlite)?;

        Ok(PaginatedThreads {
            threads,
            page,
            per_page,
            total: total as usize,
        })
    }

    /// Search threads by title. When `workspace_id` is provided, only threads
    /// whose metadata contains that workspace id are returned. An empty query
    /// returns all threads (optionally filtered by workspace).
    pub fn search_threads(
        &self,
        query: &str,
        workspace_id: Option<&str>,
    ) -> Result<Vec<ThreadMeta>, StoreError> {
        let conn = self.conn.lock();
        let threads: Vec<ThreadMeta> = if query.is_empty() {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {THREAD_SELECT_COLUMNS} FROM threads \
                     ORDER BY pinned DESC, updated_at DESC"
                ))
                .map_err(StoreError::Rusqlite)?;
            let rows = stmt
                .query_map([], row_to_thread)
                .map_err(StoreError::Rusqlite)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::Rusqlite)?
        } else {
            let q = format!("%{}%", query.to_lowercase());
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {THREAD_SELECT_COLUMNS} FROM threads \
                     WHERE lower(title) LIKE ?1 \
                     ORDER BY pinned DESC, updated_at DESC"
                ))
                .map_err(StoreError::Rusqlite)?;
            let rows = stmt
                .query_map(params![q], row_to_thread)
                .map_err(StoreError::Rusqlite)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(StoreError::Rusqlite)?
        };
        drop(conn);

        if let Some(workspace_id) = workspace_id {
            Ok(threads
                .into_iter()
                .filter(|t| {
                    t.metadata
                        .as_ref()
                        .and_then(|md| md.get("workspace_id"))
                        .and_then(|v| v.as_str())
                        == Some(workspace_id)
                })
                .collect())
        } else {
            Ok(threads)
        }
    }

    pub fn get_thread(&self, id: &str) -> Result<Option<ThreadMeta>, StoreError> {
        let conn = self.conn.lock();
        Self::get_thread_by_conn(&conn, id)
    }

    /// Update any subset of a thread's mutable fields in a single
    /// transactional `UPDATE`. Passing `Some(v)` sets the field to `v`,
    /// `Some(None)` clears it (for nullable fields), and `None` leaves the
    /// column as-is. `updated_at` is only bumped when `bump_updated_at` is
    /// `true` so that metadata-only changes (e.g. clearing the "new activity"
    /// flag) do not reorder the thread list.
    pub fn update_thread(
        &self,
        id: &str,
        title: Option<&str>,
        preview: Option<&str>,
        session_file: Option<Option<&str>>,
        model: Option<Option<&str>>,
        thinking_level: Option<Option<&str>>,
        pinned: Option<bool>,
        metadata: Option<Option<&serde_json::Value>>,
        bump_updated_at: bool,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(StoreError::Rusqlite)?;

        // The single-statement form below uses COALESCE to leave untouched
        // columns at their current value (NULL-aware for nullable ones).
        let metadata_str: Option<Option<String>> =
            metadata.map(|md| md.as_ref().map(|v| v.to_string()));

        tx.execute(
            "UPDATE threads SET
                title          = COALESCE(?2, title),
                preview        = COALESCE(?3, preview),
                session_file   = CASE WHEN ?4 IS 1 THEN ?5 ELSE session_file END,
                model          = CASE WHEN ?6 IS 1 THEN ?7 ELSE model END,
                thinking_level = CASE WHEN ?8 IS 1 THEN ?9 ELSE thinking_level END,
                pinned         = COALESCE(?10, pinned),
                metadata       = CASE WHEN ?11 IS 1 THEN ?12 ELSE metadata END,
                updated_at     = CASE WHEN ?13 IS 1 THEN datetime('now') ELSE updated_at END
             WHERE id = ?1",
            params![
                id,
                title,
                preview,
                session_file.is_some() as i32,
                session_file.flatten(),
                model.is_some() as i32,
                model.flatten(),
                thinking_level.is_some() as i32,
                thinking_level.flatten(),
                pinned.map(|p| p as i32),
                metadata_str.is_some() as i32,
                metadata_str.flatten(),
                bump_updated_at as i32,
            ],
        )
        .map_err(StoreError::Rusqlite)?;

        tx.commit().map_err(StoreError::Rusqlite)
    }

    pub fn make_unique_workspace_name(&self, base: &str) -> Result<String, StoreError> {
        let conn = self.conn.lock();
        let existing: Vec<String> = conn
            .prepare("SELECT name FROM workspaces")
            .map_err(StoreError::Rusqlite)?
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(StoreError::Rusqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Rusqlite)?;

        if !existing.iter().any(|n| n == base) {
            return Ok(base.to_string());
        }

        let mut suffix = 1;
        loop {
            let candidate = format!("{} {}", base, suffix);
            if !existing.iter().any(|n| n == &candidate) {
                return Ok(candidate);
            }
            suffix += 1;
        }
    }

    pub fn create_workspace(&self, name: &str, path: &str) -> Result<WorkspaceMeta, StoreError> {
        let unique_name = self.make_unique_workspace_name(name)?;
        let mut conn = self.conn.lock();
        let id = nanoid::nanoid!();
        let tx = conn.transaction().map_err(StoreError::Rusqlite)?;
        tx.execute(
            "INSERT INTO workspaces (id, name, path) VALUES (?1, ?2, ?3)",
            params![id, unique_name, path],
        )
        .map_err(StoreError::Rusqlite)?;
        let workspace = Self::get_workspace_by_conn(&tx, &id)?.ok_or(StoreError::MissingRow(
            format!("workspace {} after create", id),
        ))?;
        tx.commit().map_err(StoreError::Rusqlite)?;
        Ok(workspace)
    }

    pub fn list_workspaces(&self) -> Result<Vec<WorkspaceMeta>, StoreError> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, path, created_at, updated_at \
                 FROM workspaces ORDER BY updated_at DESC",
            )
            .map_err(StoreError::Rusqlite)?;
        let rows = stmt
            .query_map([], row_to_workspace)
            .map_err(StoreError::Rusqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Rusqlite)
    }

    pub fn get_workspace(&self, id: &str) -> Result<Option<WorkspaceMeta>, StoreError> {
        let conn = self.conn.lock();
        Self::get_workspace_by_conn(&conn, id)
    }

    pub fn delete_workspace(&self, id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM workspaces WHERE id = ?1", params![id])
            .map_err(StoreError::Rusqlite)?;
        Ok(())
    }

    pub fn default_workspace_dir(&self) -> PathBuf {
        self.sessions_dir
            .parent()
            .unwrap_or(&self.sessions_dir)
            .join("workspace")
    }

    pub fn toggle_pin(&self, id: &str) -> Result<bool, StoreError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(StoreError::Rusqlite)?;
        let current: bool = tx
            .query_row(
                "SELECT pinned FROM threads WHERE id = ?1",
                params![id],
                |row| row.get::<_, i32>(0),
            )
            .map(|v| v != 0)
            .map_err(StoreError::Rusqlite)?;
        let new_val = !current;
        tx.execute(
            "UPDATE threads SET pinned = ?1 WHERE id = ?2",
            params![new_val as i32, id],
        )
        .map_err(StoreError::Rusqlite)?;
        tx.commit().map_err(StoreError::Rusqlite)?;
        Ok(new_val)
    }

    pub fn delete_thread(&self, id: &str) -> Result<(), StoreError> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction().map_err(StoreError::Rusqlite)?;
        let session_file: Option<String> = tx
            .query_row(
                "SELECT session_file FROM threads WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(StoreError::Rusqlite)?
            .flatten();
        tx.execute("DELETE FROM threads WHERE id = ?1", params![id])
            .map_err(StoreError::Rusqlite)?;
        tx.commit().map_err(StoreError::Rusqlite)?;

        // Unlink the JSONL only after the row has been removed. Even though
        // the file remove is best-effort, doing it outside the transaction is
        // important: filesystem errors here must not roll the delete back (we
        // would leak the row and orphaned file), and `std::fs::remove_file`
        // is not rollback-able inside SQLite anyway.
        if let Some(sf) = session_file {
            let _ = std::fs::remove_file(self.sessions_dir.join(&sf));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct WorkspaceMeta {
    pub id: String,
    pub name: String,
    pub path: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug)]
pub enum StoreError {
    Rusqlite(rusqlite::Error),
    Io(std::io::Error),
    HomeDir,
    MissingRow(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Rusqlite(e) => write!(f, "database error: {}", e),
            StoreError::Io(e) => write!(f, "io error: {}", e),
            StoreError::HomeDir => write!(f, "could not determine home directory"),
            StoreError::MissingRow(what) => write!(f, "expected row not present: {}", what),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StoreError::Rusqlite(e) => Some(e),
            StoreError::Io(e) => Some(e),
            StoreError::HomeDir => None,
            StoreError::MissingRow(_) => None,
        }
    }
}
