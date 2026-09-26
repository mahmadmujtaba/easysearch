//! SQLite-backed index.
//!
//! One row per path, plus a few columns computed at insert time (`name`, `dir`,
//! `ext`, `is_hidden`) so the common filters are indexable:
//!
//! ```sql
//! files(id, path UNIQUE, name, dir, ext, size, mtime, is_dir, is_hidden)
//! ```
//!
//! Design notes:
//!
//! * **One writer.** The daemon owns the database and applies kernel events to
//!   it; readers (the GUI, `easysearch-cli`) only ever `SELECT`. WAL mode is on, so
//!   readers never block the writer and vice versa.
//! * **Batched writes.** Realtime updates arrive as batches and are committed in
//!   a single transaction — never one `fsync` per filesystem event.
//! * **One predicate.** The coarse filters (directory prefix, extension, size,
//!   mtime, hidden, files-only) are pushed into SQL because they are exactly the
//!   conditions the engine already applies; the name/category predicates then
//!   run in Rust through the *same* `accepts()` the mmap backend uses, so the
//!   two backends can never disagree about what matches.
//! * **Rebuild vs refresh.** The database is *created* (schema + full walk) when
//!   the file is missing or `schema_version` differs. While running, deltas are
//!   applied live and counted; when the backlog grows past
//!   [`REFRESH_AFTER_DIRTY`] the caller is expected to do a full rebuild
//!   ("refresh") instead of replaying a very long delta stream.

use crate::engine::{ResultRow, accepts};
use crate::matcher::{CompiledQuery, is_hidden};
use crate::overlay::Meta;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params_from_iter};
use std::path::{Path, PathBuf};

/// Bumped whenever the schema changes; a mismatch forces a rebuild.
pub const SCHEMA_VERSION: i64 = 1;

/// After this many live changes, a full rebuild is cheaper (and safer) than
/// replaying deltas — the callers use this as the "refresh" trigger.
pub const REFRESH_AFTER_DIRTY: u64 = 20_000;

/// File name inside the database directory.
pub const DB_FILE: &str = "index.db";

pub struct SqliteIndex {
    conn: Connection,
    /// Insert/update/delete operations applied since the last rebuild.
    dirty: u64,
}

/// The columns of one row, in insert order.
struct Row {
    path: String,
    name: String,
    dir: String,
    ext: String,
    size: i64,
    mtime: i64,
    is_dir: i64,
    hidden: i64,
}

impl SqliteIndex {
    /// Open (creating and initialising if needed) the index in `dir`.
    ///
    /// The caller checks [`SqliteIndex::needs_rebuild`] and then calls
    /// [`SqliteIndex::rebuild`] — that is the "recreate if not present" path.
    pub fn open(dir: &Path) -> Result<SqliteIndex, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(DB_FILE);
        let conn = Connection::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        // WAL lets readers run while the daemon writes; NORMAL is the usual
        // durability/speed trade-off for a rebuildable cache.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;
             PRAGMA temp_store=FILE;",
        )
        .map_err(|e| e.to_string())?;
        let mut idx = SqliteIndex { conn, dirty: 0 };
        idx.ensure_schema()?;
        Ok(idx)
    }

    fn ensure_schema(&mut self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (
                     key   TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS files (
                     id        INTEGER PRIMARY KEY,
                     path      TEXT NOT NULL UNIQUE,
                     name      TEXT NOT NULL,
                     dir       TEXT NOT NULL,
                     ext       TEXT NOT NULL DEFAULT '',
                     size      INTEGER NOT NULL DEFAULT 0,
                     mtime     INTEGER NOT NULL DEFAULT 0,
                     is_dir    INTEGER NOT NULL DEFAULT 0,
                     is_hidden INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE INDEX IF NOT EXISTS files_name ON files(name);
                 CREATE INDEX IF NOT EXISTS files_dir  ON files(dir);
                 CREATE INDEX IF NOT EXISTS files_ext  ON files(ext);
                 CREATE INDEX IF NOT EXISTS files_size ON files(size);
                 CREATE INDEX IF NOT EXISTS files_mtime ON files(mtime);",
            )
            .map_err(|e| e.to_string())?;
        let found: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        match found.as_deref().and_then(|v| v.parse::<i64>().ok()) {
            Some(v) if v == SCHEMA_VERSION => Ok(()),
            // Missing (fresh file) or from an older/newer build: start clean.
            _ => self.drop_contents(),
        }
    }

    fn drop_contents(&mut self) -> Result<(), String> {
        self.conn
            .execute_batch(&format!(
                "DELETE FROM files;
                 DELETE FROM meta WHERE key IN ('built_at', 'complete');
                 INSERT INTO meta(key, value) VALUES('schema_version', '{SCHEMA_VERSION}')
                   ON CONFLICT(key) DO UPDATE SET value = excluded.value;"
            ))
            .map_err(|e| e.to_string())
    }

    /// True once a full walk has been committed. Until then the database may
    /// hold a partial index (an interrupted first build), which is why
    /// completeness is recorded explicitly instead of being inferred from the
    /// row count: a small-but-complete index is perfectly valid.
    pub fn is_complete(&self) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM meta WHERE key = 'complete' AND value = '1'",
                [],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// True when the database cannot be trusted yet and must be built.
    pub fn needs_rebuild(&self) -> bool {
        !self.is_complete()
    }

    pub fn count_all(&self) -> Result<u64, String> {
        self.conn
            .query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0))
            .map(|n| n.max(0) as u64)
            .map_err(|e| e.to_string())
    }

    /// (files, dirs) currently in the database.
    pub fn counts(&self) -> Result<(u64, u64), String> {
        self.conn
            .query_row(
                "SELECT sum(is_dir = 0), sum(is_dir = 1) FROM files",
                [],
                |r| {
                    Ok((
                        r.get::<_, Option<i64>>(0)?.unwrap_or(0).max(0) as u64,
                        r.get::<_, Option<i64>>(1)?.unwrap_or(0).max(0) as u64,
                    ))
                },
            )
            .map_err(|e| e.to_string())
    }

    /// Replace the whole table with `entries` (a fresh walk).
    pub fn rebuild(&mut self, entries: &[(PathBuf, Meta)]) -> Result<(), String> {
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM files", [])
            .map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO files(path, name, dir, ext, size, mtime, is_dir, is_hidden)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(path) DO UPDATE SET
                         name = excluded.name,
                         dir = excluded.dir,
                         ext = excluded.ext,
                         size = excluded.size,
                         mtime = excluded.mtime,
                         is_dir = excluded.is_dir,
                         is_hidden = excluded.is_hidden",
                )
                .map_err(|e| e.to_string())?;
            for (path, meta) in entries {
                let row = Row::new(path, *meta);
                stmt.execute(params_from_iter(row.as_values().iter()))
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES('built_at', strftime('%s','now'))
               ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .map_err(|e| e.to_string())?;
        // Marked complete in the same transaction as the rows, so an aborted
        // build can never look finished.
        tx.execute(
            "INSERT INTO meta(key, value) VALUES('complete', '1')
               ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        self.dirty = 0;
        Ok(())
    }

    /// Apply one batch of changes from the watcher, in a single transaction.
    pub fn apply(&mut self, added: &[(PathBuf, Meta)], removed: &[PathBuf]) -> Result<(), String> {
        if added.is_empty() && removed.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut upsert = tx
                .prepare_cached(
                    "INSERT INTO files(path, name, dir, ext, size, mtime, is_dir, is_hidden)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(path) DO UPDATE SET
                         name = excluded.name,
                         dir = excluded.dir,
                         ext = excluded.ext,
                         size = excluded.size,
                         mtime = excluded.mtime,
                         is_dir = excluded.is_dir,
                         is_hidden = excluded.is_hidden",
                )
                .map_err(|e| e.to_string())?;
            for (path, meta) in added {
                let row = Row::new(path, *meta);
                upsert
                    .execute(params_from_iter(row.as_values().iter()))
                    .map_err(|e| e.to_string())?;
            }
        }
        {
            let mut del = tx
                .prepare_cached("DELETE FROM files WHERE path = ?1")
                .map_err(|e| e.to_string())?;
            for path in removed {
                del.execute([path.to_string_lossy().as_ref()])
                    .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        self.dirty = self
            .dirty
            .saturating_add((added.len() + removed.len()) as u64);
        Ok(())
    }

    /// Changes applied since the last rebuild — the "refresh" signal.
    pub fn dirty(&self) -> u64 {
        self.dirty
    }

    /// Rows matching `cq`, up to `cq.limit`.
    ///
    /// The SQL narrows what it provably can; every candidate is then checked
    /// with the engine's own [`accepts`] so the result set is identical to the
    /// mmap backend's.
    pub fn search(&self, cq: &CompiledQuery) -> Result<Vec<ResultRow>, String> {
        let files_only = cq.content.is_some();
        let (where_sql, args) = coarse_sql(cq, files_only);
        let sql = format!("SELECT path, size, mtime, is_dir FROM files {where_sql} ORDER BY id");
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let size: i64 = row.get(1).map_err(|e| e.to_string())?;
            let mtime: i64 = row.get(2).map_err(|e| e.to_string())?;
            let is_dir: i64 = row.get(3).map_err(|e| e.to_string())?;
            let p = PathBuf::from(&path);
            let meta = Meta {
                size: size.max(0) as u64,
                mtime,
                is_dir: is_dir != 0,
            };
            if !accepts(cq, &p, meta, files_only) {
                continue;
            }
            out.push(ResultRow {
                path: p,
                size: meta.size,
                mtime: meta.mtime,
                is_dir: meta.is_dir,
            });
            if out.len() >= cq.limit {
                break;
            }
        }
        Ok(out)
    }

    /// Matching paths only — the candidate list a content search fans out over.
    pub fn candidates(&self, cq: &CompiledQuery, cap: usize) -> Result<Vec<PathBuf>, String> {
        let files_only = cq.content.is_some();
        let (where_sql, args) = coarse_sql(cq, files_only);
        let sql = format!("SELECT path, size, mtime, is_dir FROM files {where_sql} ORDER BY id");
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let size: i64 = row.get(1).map_err(|e| e.to_string())?;
            let mtime: i64 = row.get(2).map_err(|e| e.to_string())?;
            let is_dir: i64 = row.get(3).map_err(|e| e.to_string())?;
            let p = PathBuf::from(path);
            if accepts(
                cq,
                &p,
                Meta {
                    size: size.max(0) as u64,
                    mtime,
                    is_dir: is_dir != 0,
                },
                files_only,
            ) {
                out.push(p);
                if out.len() >= cap {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Metadata for one path, if it is indexed.
    pub fn meta_of(&self, path: &Path) -> Option<Meta> {
        self.conn
            .query_row(
                "SELECT size, mtime, is_dir FROM files WHERE path = ?1",
                [path.to_string_lossy().as_ref()],
                |r| {
                    Ok(Meta {
                        size: r.get::<_, i64>(0)?.max(0) as u64,
                        mtime: r.get::<_, i64>(1)?,
                        is_dir: r.get::<_, i64>(2)? != 0,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM files WHERE path = ?1 LIMIT 1",
                [path.to_string_lossy().as_ref()],
                |_| Ok(()),
            )
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// How many rows match — the SQLite twin of `Engine::count`.
    pub fn count(&self, cq: &CompiledQuery) -> Result<u64, String> {
        let files_only = cq.content.is_some();
        let (where_sql, args) = coarse_sql(cq, files_only);
        let sql = format!("SELECT path, size, mtime, is_dir FROM files {where_sql} ORDER BY id");
        let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(params_from_iter(args.iter()))
            .map_err(|e| e.to_string())?;
        let mut n = 0u64;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let path: String = row.get(0).map_err(|e| e.to_string())?;
            let size: i64 = row.get(1).map_err(|e| e.to_string())?;
            let mtime: i64 = row.get(2).map_err(|e| e.to_string())?;
            let is_dir: i64 = row.get(3).map_err(|e| e.to_string())?;
            if accepts(
                cq,
                Path::new(&path),
                Meta {
                    size: size.max(0) as u64,
                    mtime,
                    is_dir: is_dir != 0,
                },
                files_only,
            ) {
                n += 1;
            }
        }
        Ok(n)
    }
}

impl Row {
    fn new(path: &Path, meta: Meta) -> Row {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dir = path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ext = if meta.is_dir {
            String::new()
        } else {
            path.extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default()
        };
        Row {
            path: path.to_string_lossy().into_owned(),
            name,
            dir,
            ext,
            size: meta.size as i64,
            mtime: meta.mtime,
            is_dir: i64::from(meta.is_dir),
            hidden: i64::from(is_hidden(path)),
        }
    }

    fn as_values(&self) -> [Value; 8] {
        [
            Value::Text(self.path.clone()),
            Value::Text(self.name.clone()),
            Value::Text(self.dir.clone()),
            Value::Text(self.ext.clone()),
            Value::Integer(self.size),
            Value::Integer(self.mtime),
            Value::Integer(self.is_dir),
            Value::Integer(self.hidden),
        ]
    }
}

/// The pushed-down part of a query: exactly the conditions [`accepts`] applies
/// that are expressible as plain column comparisons.
fn coarse_sql(cq: &CompiledQuery, files_only: bool) -> (String, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut args: Vec<Value> = Vec::new();

    // Files-only sources of truth.
    if files_only || !cq.include_dirs || !cq.extensions.is_empty() {
        clauses.push("is_dir = 0".to_string());
    }
    if !cq.extensions.is_empty() {
        let holes = vec!["?"; cq.extensions.len()].join(",");
        clauses.push(format!("ext IN ({holes})"));
        for e in &cq.extensions {
            args.push(Value::Text(e.clone()));
        }
    }
    if cq.min_size.is_some() || cq.max_size.is_some() {
        push_files_only(&mut clauses);
    }
    if let Some(min) = cq.min_size {
        clauses.push("size >= ?".to_string());
        args.push(Value::Integer(min as i64));
    }
    if let Some(max) = cq.max_size {
        clauses.push("size <= ?".to_string());
        args.push(Value::Integer(max as i64));
    }
    if let Some(secs) = cq.modified_within_secs {
        clauses.push("mtime >= ?".to_string());
        args.push(Value::Integer(now_unix() - secs));
    }
    if !cq.include_hidden {
        clauses.push("is_hidden = 0".to_string());
    }
    if let Some(prefix) = cq.under.as_deref() {
        // `Path::starts_with` compares whole components, so a path matches when
        // it *is* the prefix or continues with a separator.
        clauses.push("(path = ? OR substr(path, 1, ?) = ?)".to_string());
        args.push(Value::Text(prefix.to_string()));
        args.push(Value::Integer(prefix.chars().count() as i64 + 1));
        args.push(Value::Text(format!("{prefix}/")));
    }

    if clauses.is_empty() {
        (String::new(), args)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), args)
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Add `is_dir = 0` unless an earlier clause already excludes directories.
fn push_files_only(clauses: &mut Vec<String>) {
    if !clauses.iter().any(|c| c == "is_dir = 0") {
        clauses.push("is_dir = 0".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::Query;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("efl-sqlite-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A small tree: 3 files + 1 directory, one hidden.
    fn sample() -> Vec<(PathBuf, Meta)> {
        let now = now_unix();
        vec![
            (
                PathBuf::from("/home/u/docs/report.pdf"),
                Meta {
                    size: 5_000,
                    mtime: now - 60,
                    is_dir: false,
                },
            ),
            (
                PathBuf::from("/home/u/docs/notes.md"),
                Meta {
                    size: 40,
                    mtime: now - 90_000,
                    is_dir: false,
                },
            ),
            (
                PathBuf::from("/home/u/docs/.secret.pdf"),
                Meta {
                    size: 10,
                    mtime: now - 30,
                    is_dir: false,
                },
            ),
            (
                PathBuf::from("/home/u/docs/sub"),
                Meta {
                    size: 0,
                    mtime: now - 30,
                    is_dir: true,
                },
            ),
        ]
    }

    fn idx(tag: &str) -> (SqliteIndex, PathBuf) {
        let dir = tmpdir(tag);
        let mut i = SqliteIndex::open(&dir).unwrap();
        assert!(i.needs_rebuild(), "a fresh database needs a rebuild");
        i.rebuild(&sample()).unwrap();
        assert!(!i.needs_rebuild());
        (i, dir)
    }

    fn query(f: impl FnOnce(&mut Query)) -> Query {
        let mut q = Query {
            limit: 1000,
            include_dirs: true,
            ..Query::default()
        };
        f(&mut q);
        q
    }

    fn run(i: &SqliteIndex, q: &Query) -> Vec<String> {
        let cq = CompiledQuery::compile(q).unwrap();
        let mut v: Vec<String> = i
            .search(&cq)
            .unwrap()
            .into_iter()
            .map(|r| r.path.to_string_lossy().into_owned())
            .collect();
        v.sort();
        // `count` must agree with `search` for every query.
        assert_eq!(i.count(&cq).unwrap(), v.len() as u64);
        v
    }

    #[test]
    fn rebuild_then_search_and_count() {
        let (i, dir) = idx("basic");
        assert_eq!(i.count_all().unwrap(), 4);
        assert_eq!(i.counts().unwrap(), (3, 1));
        assert!(!i.needs_rebuild());
        assert_eq!(
            run(&i, &query(|_| {})).len(),
            3,
            "hidden filtered by default"
        );
        let all = run(&i, &query(|q| q.include_hidden = true));
        assert_eq!(all.len(), 4);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn name_glob_and_exclusions_use_the_shared_predicate() {
        let (i, dir) = idx("name");
        let pdfs = run(&i, &query(|q| q.name = "*.pdf".into()));
        assert_eq!(pdfs, vec!["/home/u/docs/report.pdf"]);
        let not_md = run(&i, &query(|q| q.name = "!*.md".into()));
        assert!(!not_md.iter().any(|p| p.ends_with("notes.md")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn extension_size_and_recency_filters() {
        let (i, dir) = idx("filters");
        assert_eq!(
            run(&i, &query(|q| q.extensions = vec!["pdf".into()])).len(),
            1
        );
        // The extension filter is files-only, so the directory is out too.
        assert_eq!(
            run(&i, &query(|q| q.extensions = vec!["md".into()])).len(),
            1
        );
        let big = run(&i, &query(|q| q.min_size = Some(1000)));
        assert_eq!(big, vec!["/home/u/docs/report.pdf"]);
        let small = run(&i, &query(|q| q.max_size = Some(100)));
        assert_eq!(small, vec!["/home/u/docs/notes.md"]);
        // Recency: only the two touched a minute ago.
        let recent = run(&i, &query(|q| q.modified_within_secs = Some(3600)));
        assert_eq!(recent.len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn under_is_component_wise() {
        let (i, dir) = idx("under");
        let docs = run(&i, &query(|q| q.under = Some("/home/u/docs".into())));
        assert_eq!(docs.len(), 3);
        // A prefix that is not a whole component must not match.
        let none = run(&i, &query(|q| q.under = Some("/home/u/doc".into())));
        assert!(none.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn content_queries_exclude_directories() {
        let (i, dir) = idx("content");
        let cq = CompiledQuery::compile(&query(|q| {
            q.content = Some("needle".into());
            q.name = String::new();
        }))
        .unwrap();
        assert!(i.search(&cq).unwrap().iter().all(|r| !r.is_dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn deltas_are_applied_and_counted() {
        let (mut i, dir) = idx("delta");
        let now = now_unix();
        i.apply(
            &[
                (
                    PathBuf::from("/home/u/docs/new.pdf"),
                    Meta {
                        size: 7,
                        mtime: now,
                        is_dir: false,
                    },
                ),
                (
                    PathBuf::from("/home/u/docs/report.pdf"),
                    Meta {
                        size: 9_999,
                        mtime: now,
                        is_dir: false,
                    },
                ),
            ],
            &[PathBuf::from("/home/u/docs/notes.md")],
        )
        .unwrap();
        assert_eq!(i.dirty(), 3);

        let pdfs = run(&i, &query(|q| q.name = "*.pdf".into()));
        assert_eq!(
            pdfs,
            vec!["/home/u/docs/new.pdf", "/home/u/docs/report.pdf"],
            "insert and update both landed"
        );
        // The update changed the size, so the size filter must see the new value.
        let big = run(&i, &query(|q| q.min_size = Some(9_000)));
        assert_eq!(big, vec!["/home/u/docs/report.pdf"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_partial_build_is_not_treated_as_complete() {
        // An interrupted first build leaves rows behind; `complete` is what
        // says whether they can be trusted.
        let dir = tmpdir("partial");
        {
            let mut i = SqliteIndex::open(&dir).unwrap();
            i.rebuild(&sample()).unwrap();
            assert!(!i.needs_rebuild());
            i.conn
                .execute("DELETE FROM meta WHERE key = 'complete'", [])
                .unwrap();
        }
        let reopened = SqliteIndex::open(&dir).unwrap();
        assert_eq!(reopened.count_all().unwrap(), 4, "rows survived");
        assert!(
            reopened.needs_rebuild(),
            "but the index must be rebuilt before it is served"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_empty_but_complete_index_is_valid() {
        let dir = tmpdir("empty");
        let mut i = SqliteIndex::open(&dir).unwrap();
        i.rebuild(&[]).unwrap();
        assert!(
            !i.needs_rebuild(),
            "a legitimately empty root must not rebuild on every start"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn schema_mismatch_forces_a_rebuild() {
        let dir = tmpdir("schema");
        {
            let mut i = SqliteIndex::open(&dir).unwrap();
            i.rebuild(&sample()).unwrap();
            i.conn
                .execute(
                    "UPDATE meta SET value = '99' WHERE key = 'schema_version'",
                    [],
                )
                .unwrap();
        }
        let reopened = SqliteIndex::open(&dir).unwrap();
        assert!(
            reopened.needs_rebuild(),
            "an incompatible schema must come back empty and be rebuilt"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
