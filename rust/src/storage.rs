//! Index storage with save/load, byte-offset content retrieval, and incremental indexing.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::parser::symbols::Symbol;

/// Bump when schema changes incompatibly.
const INDEX_VERSION: i64 = 4;

/// Bytes per token estimate for savings tracking.
const BYTES_PER_TOKEN: usize = 4;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn file_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn git_head(repo_path: &Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
}

fn safe_repo_component(value: &str) -> Result<&str> {
    if value.is_empty() || value == "." || value == ".." {
        anyhow::bail!("invalid repo component: {value:?}");
    }
    if value.contains('/') || value.contains('\\') {
        anyhow::bail!("invalid repo component: {value:?}");
    }
    if !value.chars().all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-') {
        anyhow::bail!("invalid repo component: {value:?}");
    }
    Ok(value)
}

/// Convert an FTS5 plain-text query to safe MATCH syntax.
///
/// Each token is quoted to prevent FTS5 operator injection.
pub fn fts5_query(query: &str) -> String {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }
    tokens
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', " ").trim()))
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS repo_meta (
            repo           TEXT NOT NULL,
            owner          TEXT NOT NULL,
            name           TEXT NOT NULL,
            indexed_at     TEXT NOT NULL,
            index_version  INTEGER NOT NULL,
            git_head       TEXT NOT NULL DEFAULT '',
            source_files   TEXT NOT NULL DEFAULT '[]',
            languages      TEXT NOT NULL DEFAULT '{}',
            file_hashes    TEXT NOT NULL DEFAULT '{}',
            file_summaries TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE IF NOT EXISTS symbols (
            id             TEXT PRIMARY KEY,
            file           TEXT NOT NULL,
            name           TEXT NOT NULL,
            qualified_name TEXT NOT NULL DEFAULT '',
            kind           TEXT NOT NULL,
            language       TEXT NOT NULL DEFAULT '',
            signature      TEXT NOT NULL DEFAULT '',
            docstring      TEXT NOT NULL DEFAULT '',
            summary        TEXT NOT NULL DEFAULT '',
            decorators     TEXT NOT NULL DEFAULT '[]',
            keywords       TEXT NOT NULL DEFAULT '[]',
            parent         TEXT,
            line           INTEGER NOT NULL DEFAULT 0,
            end_line       INTEGER NOT NULL DEFAULT 0,
            byte_offset    INTEGER NOT NULL DEFAULT 0,
            byte_length    INTEGER NOT NULL DEFAULT 0,
            content_hash   TEXT NOT NULL DEFAULT ''
        );

        CREATE INDEX IF NOT EXISTS idx_symbols_file     ON symbols(file);
        CREATE INDEX IF NOT EXISTS idx_symbols_kind     ON symbols(kind);
        CREATE INDEX IF NOT EXISTS idx_symbols_language ON symbols(language);
        CREATE INDEX IF NOT EXISTS idx_symbols_name     ON symbols(name);

        CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
            id UNINDEXED,
            name,
            signature,
            summary,
            docstring,
            content='symbols',
            content_rowid='rowid'
        );

        CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO symbols_fts(rowid, id, name, signature, summary, docstring)
            VALUES (new.rowid, new.id, new.name, new.signature, new.summary, new.docstring);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO symbols_fts(symbols_fts, rowid, id, name, signature, summary, docstring)
            VALUES ('delete', old.rowid, old.id, old.name, old.signature, old.summary, old.docstring);
        END;

        CREATE TABLE IF NOT EXISTS imports (
            from_file   TEXT NOT NULL,
            import_path TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_imports_from ON imports(from_file);

        CREATE TABLE IF NOT EXISTS proto_refs (
            from_symbol_id TEXT NOT NULL,
            to_type_name   TEXT NOT NULL,
            field_name     TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_proto_refs_from ON proto_refs(from_symbol_id);

        CREATE TABLE IF NOT EXISTS impl_refs (
            from_symbol_id TEXT NOT NULL,
            to_type_name   TEXT NOT NULL,
            kind           TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_impl_refs_from ON impl_refs(from_symbol_id);
        CREATE INDEX IF NOT EXISTS idx_impl_refs_to   ON impl_refs(to_type_name);
        ",
    )?;
    Ok(())
}

fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL")?;
    ensure_schema(&conn)?;
    Ok(conn)
}

// ---------------------------------------------------------------------------
// Bulk insert helpers
// ---------------------------------------------------------------------------

fn insert_symbols(conn: &Connection, symbols: &[Symbol]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO symbols
         (id, file, name, qualified_name, kind, language, signature, docstring, summary,
          decorators, keywords, parent, line, end_line, byte_offset, byte_length, content_hash)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )?;

    for s in symbols {
        let decorators_json = serde_json::to_string(&s.decorators)?;
        let keywords_json = "[]"; // keywords populated later by AI summarizer
        stmt.execute(params![
            s.id,
            s.file,
            s.name,
            s.qualified_name,
            s.kind,
            s.language,
            s.signature,
            s.docstring,
            s.summary,
            decorators_json,
            keywords_json,
            s.parent,
            s.line,
            s.end_line,
            s.byte_offset,
            s.byte_length,
            s.content_hash,
        ])?;
    }
    Ok(())
}

fn insert_imports(conn: &Connection, imports: &[(String, Vec<String>)]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO imports (from_file, import_path) VALUES (?, ?)",
    )?;
    for (file, paths) in imports {
        for path in paths {
            stmt.execute(params![file, path])?;
        }
    }
    Ok(())
}

fn insert_proto_refs(conn: &Connection, refs: &[crate::parser::proto_refs::ProtoRef]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO proto_refs (from_symbol_id, to_type_name, field_name) VALUES (?, ?, ?)",
    )?;
    for r in refs {
        stmt.execute(params![r.from_symbol_id, r.to_type_name, r.field_name])?;
    }
    Ok(())
}

fn insert_impl_refs(conn: &Connection, refs: &[crate::parser::impl_refs::ImplRef]) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO impl_refs (from_symbol_id, to_type_name, kind) VALUES (?, ?, ?)",
    )?;
    for r in refs {
        stmt.execute(params![r.from_symbol_id, r.to_type_name, r.kind])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// IndexStore
// ---------------------------------------------------------------------------

/// SQLite-backed storage for code indexes with byte-offset content retrieval.
pub struct IndexStore {
    base_path: PathBuf,
}

impl IndexStore {
    /// Open the index store, creating the base directory if needed.
    pub fn open_store(base_path: Option<&Path>) -> Result<Self> {
        let base = match base_path {
            Some(p) => p.to_path_buf(),
            None => Config::from_env().index_path,
        };
        std::fs::create_dir_all(&base)?;
        Ok(Self { base_path: base })
    }

    fn repo_slug(&self, owner: &str, name: &str) -> Result<String> {
        let o = safe_repo_component(owner)?;
        let n = safe_repo_component(name)?;
        Ok(format!("{o}-{n}"))
    }

    fn index_path(&self, owner: &str, name: &str) -> Result<PathBuf> {
        Ok(self.base_path.join(format!("{}.db", self.repo_slug(owner, name)?)))
    }

    /// Public accessor for tools that need the DB path directly.
    pub fn index_path_pub(&self, owner: &str, name: &str) -> Result<PathBuf> {
        self.index_path(owner, name)
    }

    fn content_dir(&self, owner: &str, name: &str) -> Result<PathBuf> {
        Ok(self.base_path.join(self.repo_slug(owner, name)?))
    }

    /// Public accessor for tools that need the content directory.
    pub fn content_dir_pub(&self, owner: &str, name: &str) -> Result<PathBuf> {
        self.content_dir(owner, name)
    }

    /// Validate that a content path stays within the content directory.
    fn safe_content_path(&self, content_dir: &Path, relative: &str) -> Option<PathBuf> {
        let base = content_dir.canonicalize().ok()?;
        let candidate = content_dir.join(relative);
        // Create parent dirs first so canonicalize works for new files.
        if let Some(parent) = candidate.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        // For new files, canonicalize the parent and append the filename.
        let parent_canon = candidate.parent()?.canonicalize().ok()?;
        let full = parent_canon.join(candidate.file_name()?);
        if full.starts_with(&base) {
            Some(full)
        } else {
            None
        }
    }

    // --- Save ---

    /// Save a full index (initial indexing).
    pub fn save_index(
        &self,
        owner: &str,
        name: &str,
        source_files: &[String],
        symbols: &[Symbol],
        raw_files: &HashMap<String, String>,
        languages: &HashMap<String, usize>,
        file_hashes: Option<&HashMap<String, String>>,
        repo_path: Option<&Path>,
        imports: &[(String, Vec<String>)],
        proto_refs: &[crate::parser::proto_refs::ProtoRef],
        impl_refs: &[crate::parser::impl_refs::ImplRef],
    ) -> Result<()> {
        let computed_hashes: HashMap<String, String>;
        let hashes = match file_hashes {
            Some(h) => h,
            None => {
                computed_hashes = raw_files
                    .iter()
                    .map(|(fp, content)| (fp.clone(), file_hash(content)))
                    .collect();
                &computed_hashes
            }
        };

        let indexed_at = chrono_now();
        let head = repo_path.and_then(git_head).unwrap_or_default();

        // Write to .db.tmp then rename atomically.
        let db_path = self.index_path(owner, name)?;
        let tmp_path = db_path.with_extension("db.tmp");
        if tmp_path.exists() {
            std::fs::remove_file(&tmp_path)?;
        }

        let conn = open_db(&tmp_path)?;
        conn.execute(
            "INSERT INTO repo_meta
             (repo, owner, name, indexed_at, index_version, git_head,
              source_files, languages, file_hashes, file_summaries)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                format!("{owner}/{name}"),
                owner,
                name,
                indexed_at,
                INDEX_VERSION,
                head,
                serde_json::to_string(source_files)?,
                serde_json::to_string(languages)?,
                serde_json::to_string(hashes)?,
                "{}",
            ],
        )?;

        insert_symbols(&conn, symbols)?;
        insert_imports(&conn, imports)?;
        insert_proto_refs(&conn, proto_refs)?;
        insert_impl_refs(&conn, impl_refs)?;
        drop(conn);

        std::fs::rename(&tmp_path, &db_path)?;

        // Save raw files for byte-offset retrieval.
        let cdir = self.content_dir(owner, name)?;
        std::fs::create_dir_all(&cdir)?;
        for (file_path, content) in raw_files {
            let dest = self
                .safe_content_path(&cdir, file_path)
                .context(format!("unsafe file path: {file_path}"))?;
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, content)?;
        }

        Ok(())
    }

    // --- Load ---

    /// Check if an index exists.
    pub fn index_exists(&self, owner: &str, name: &str) -> bool {
        self.index_path(owner, name)
            .map(|p| p.exists())
            .unwrap_or(false)
    }

    /// Get symbol count without loading the full index.
    pub fn count_symbols(&self, owner: &str, name: &str) -> Result<usize> {
        let db_path = self.index_path(owner, name)?;
        if !db_path.exists() {
            return Ok(0);
        }
        let conn = Connection::open(&db_path)?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
        Ok(count as usize)
    }

    /// Read symbol source using stored byte offsets. O(1).
    pub fn get_symbol_content(&self, owner: &str, name: &str, symbol_id: &str) -> Result<Option<String>> {
        let db_path = self.index_path(owner, name)?;
        if !db_path.exists() {
            return Ok(None);
        }

        let conn = Connection::open(&db_path)?;
        let row: Option<(String, i64, i64)> = conn
            .query_row(
                "SELECT file, byte_offset, byte_length FROM symbols WHERE id = ?",
                params![symbol_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let (file, offset, length) = match row {
            Some(r) => r,
            None => return Ok(None),
        };

        let cdir = self.content_dir(owner, name)?;
        let file_path = match self.safe_content_path(&cdir, &file) {
            Some(p) if p.exists() => p,
            _ => return Ok(None),
        };

        let data = std::fs::read(&file_path)?;
        let start = offset as usize;
        let end = (offset + length) as usize;
        if end > data.len() {
            return Ok(None);
        }

        Ok(Some(String::from_utf8_lossy(&data[start..end]).to_string()))
    }

    // --- Incremental ---

    /// Detect changed, new, and deleted files by comparing SHA-256 hashes.
    pub fn detect_changes(
        &self,
        owner: &str,
        name: &str,
        current_files: &HashMap<String, String>,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        let db_path = self.index_path(owner, name)?;
        if !db_path.exists() {
            return Ok((Vec::new(), current_files.keys().cloned().collect(), Vec::new()));
        }

        let conn = Connection::open(&db_path)?;
        let old_hashes_json: String = conn.query_row(
            "SELECT file_hashes FROM repo_meta LIMIT 1",
            [],
            |r| r.get(0),
        )?;
        let old_hashes: HashMap<String, String> = serde_json::from_str(&old_hashes_json)?;

        let current_hashes: HashMap<String, String> = current_files
            .iter()
            .map(|(fp, content)| (fp.clone(), file_hash(content)))
            .collect();

        let old_set: HashSet<&String> = old_hashes.keys().collect();
        let new_set: HashSet<&String> = current_hashes.keys().collect();

        let new_files: Vec<String> = new_set.difference(&old_set).map(|s| (*s).clone()).collect();
        let deleted: Vec<String> = old_set.difference(&new_set).map(|s| (*s).clone()).collect();
        let changed: Vec<String> = old_set
            .intersection(&new_set)
            .filter(|fp| old_hashes[**fp] != current_hashes[**fp])
            .map(|s| (*s).clone())
            .collect();

        Ok((changed, new_files, deleted))
    }

    /// Incrementally update an existing index.
    pub fn incremental_save(
        &self,
        owner: &str,
        name: &str,
        changed_files: &[String],
        new_files: &[String],
        deleted_files: &[String],
        new_symbols: &[Symbol],
        raw_files: &HashMap<String, String>,
        languages: &HashMap<String, usize>,
        repo_path: Option<&Path>,
        imports: &[(String, Vec<String>)],
        proto_refs: &[crate::parser::proto_refs::ProtoRef],
        impl_refs: &[crate::parser::impl_refs::ImplRef],
    ) -> Result<()> {
        let db_path = self.index_path(owner, name)?;
        if !db_path.exists() {
            anyhow::bail!("no existing index to update incrementally");
        }

        let head = repo_path.and_then(git_head).unwrap_or_default();
        let conn = Connection::open(&db_path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL")?;

        // Load current metadata.
        let (source_files_json, file_hashes_json): (String, String) = conn.query_row(
            "SELECT source_files, file_hashes FROM repo_meta LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let mut source_files: HashSet<String> = serde_json::from_str(&source_files_json)?;
        let mut hashes: HashMap<String, String> = serde_json::from_str(&file_hashes_json)?;

        // Remove symbols for deleted + changed files.
        let files_to_remove: Vec<&str> = deleted_files
            .iter()
            .chain(changed_files.iter())
            .map(|s| s.as_str())
            .collect();

        if !files_to_remove.is_empty() {
            let placeholders = files_to_remove.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!("DELETE FROM symbols WHERE file IN ({placeholders})");
            let mut stmt = conn.prepare(&sql)?;
            for (i, f) in files_to_remove.iter().enumerate() {
                stmt.raw_bind_parameter(i + 1, *f)?;
            }
            stmt.raw_execute()?;

            let sql2 = format!("DELETE FROM imports WHERE from_file IN ({placeholders})");
            let mut stmt2 = conn.prepare(&sql2)?;
            for (i, f) in files_to_remove.iter().enumerate() {
                stmt2.raw_bind_parameter(i + 1, *f)?;
            }
            stmt2.raw_execute()?;

            // Delete proto_refs and impl_refs for affected files (by file prefix match).
            for f in &files_to_remove {
                conn.execute(
                    "DELETE FROM proto_refs WHERE from_symbol_id LIKE ?",
                    [format!("{f}::%")],
                )?;
                conn.execute(
                    "DELETE FROM impl_refs WHERE from_symbol_id LIKE ?",
                    [format!("{f}::%")],
                )?;
            }
        }

        // Insert new symbols, imports, proto refs, and impl refs.
        insert_symbols(&conn, new_symbols)?;
        insert_imports(&conn, imports)?;
        insert_proto_refs(&conn, proto_refs)?;
        insert_impl_refs(&conn, impl_refs)?;

        // Update source files set.
        for f in deleted_files {
            source_files.remove(f);
        }
        for f in new_files.iter().chain(changed_files.iter()) {
            source_files.insert(f.clone());
        }

        // Update file hashes.
        for f in deleted_files {
            hashes.remove(f);
        }
        for (fp, content) in raw_files {
            hashes.insert(fp.clone(), file_hash(content));
        }

        let mut sorted_files: Vec<String> = source_files.into_iter().collect();
        sorted_files.sort();

        conn.execute(
            "UPDATE repo_meta SET indexed_at=?, source_files=?, languages=?, file_hashes=?, git_head=?",
            params![
                chrono_now(),
                serde_json::to_string(&sorted_files)?,
                serde_json::to_string(languages)?,
                serde_json::to_string(&hashes)?,
                head,
            ],
        )?;

        drop(conn);

        // Update raw files on disk.
        let cdir = self.content_dir(owner, name)?;
        std::fs::create_dir_all(&cdir)?;

        for fp in deleted_files {
            if let Some(dead) = self.safe_content_path(&cdir, fp) {
                let _ = std::fs::remove_file(dead);
            }
        }
        for (fp, content) in raw_files {
            let dest = self
                .safe_content_path(&cdir, fp)
                .context(format!("unsafe file path: {fp}"))?;
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, content)?;
        }

        Ok(())
    }

    // --- Search ---

    /// FTS5 BM25-ranked symbol search.
    pub fn search_fts(
        &self,
        owner: &str,
        name: &str,
        query: &str,
        kind: Option<&str>,
        language: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SymbolRow>> {
        let db_path = self.index_path(owner, name)?;
        if !db_path.exists() {
            return Ok(Vec::new());
        }

        let fts = fts5_query(query);
        if fts.is_empty() {
            return Ok(Vec::new());
        }

        let mut wheres = vec!["symbols_fts MATCH ?1".to_string()];
        let mut param_idx = 2;

        if kind.is_some() {
            wheres.push(format!("s.kind = ?{param_idx}"));
            param_idx += 1;
        }
        if language.is_some() {
            wheres.push(format!("s.language = ?{param_idx}"));
        }

        let where_clause = wheres.join(" AND ");
        let sql = format!(
            "SELECT s.id, s.file, s.name, s.qualified_name, s.kind, s.language,
                    s.signature, s.docstring, s.summary, s.decorators, s.keywords,
                    s.parent, s.line, s.end_line, s.byte_offset, s.byte_length, s.content_hash
             FROM symbols_fts JOIN symbols s ON s.rowid = symbols_fts.rowid
             WHERE {where_clause}
             ORDER BY symbols_fts.rank
             LIMIT ?",
        );

        let conn = Connection::open(&db_path)?;
        let mut stmt = conn.prepare(&sql)?;

        // Bind parameters dynamically.
        let mut idx = 1;
        stmt.raw_bind_parameter(idx, &fts)?;
        idx += 1;
        if let Some(k) = kind {
            stmt.raw_bind_parameter(idx, k)?;
            idx += 1;
        }
        if let Some(l) = language {
            stmt.raw_bind_parameter(idx, l)?;
            idx += 1;
        }
        stmt.raw_bind_parameter(idx, limit as i64)?;

        let mut rows = Vec::new();
        let mut raw_rows = stmt.raw_query();
        while let Some(r) = raw_rows.next()? {
            rows.push(SymbolRow {
                id: r.get(0)?,
                file: r.get(1)?,
                name: r.get(2)?,
                qualified_name: r.get(3)?,
                kind: r.get(4)?,
                language: r.get(5)?,
                signature: r.get(6)?,
                docstring: r.get(7)?,
                summary: r.get(8)?,
                decorators_json: r.get(9)?,
                keywords_json: r.get(10)?,
                parent: r.get(11)?,
                line: r.get(12)?,
                end_line: r.get(13)?,
                byte_offset: r.get(14)?,
                byte_length: r.get(15)?,
                content_hash: r.get(16)?,
            });
        }

        Ok(rows)
    }

    // --- List / Delete ---

    /// List all indexed repositories.
    pub fn list_repos(&self) -> Result<Vec<RepoInfo>> {
        let mut repos = Vec::new();

        for entry in std::fs::read_dir(&self.base_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("db") {
                continue;
            }
            if path.to_string_lossy().ends_with(".db.tmp") {
                continue;
            }

            let conn = match Connection::open(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let meta = conn
                .query_row(
                    "SELECT repo, indexed_at, languages, source_files, index_version FROM repo_meta LIMIT 1",
                    [],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, i64>(4)?,
                        ))
                    },
                );

            if let Ok((repo, indexed_at, langs_json, files_json, version)) = meta {
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
                    .unwrap_or(0);
                let files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
                let languages: HashMap<String, usize> =
                    serde_json::from_str(&langs_json).unwrap_or_default();

                repos.push(RepoInfo {
                    repo,
                    indexed_at,
                    symbol_count: count as usize,
                    file_count: files.len(),
                    languages,
                    index_version: version,
                });
            }
        }

        Ok(repos)
    }

    /// Delete an index and its raw files.
    pub fn delete_index(&self, owner: &str, name: &str) -> Result<bool> {
        let db = self.index_path(owner, name)?;
        let cdir = self.content_dir(owner, name)?;
        let mut deleted = false;

        if db.exists() {
            std::fs::remove_file(&db)?;
            deleted = true;
        }
        if cdir.exists() {
            std::fs::remove_dir_all(&cdir)?;
            deleted = true;
        }

        Ok(deleted)
    }
}

// ---------------------------------------------------------------------------
// Data types returned by queries
// ---------------------------------------------------------------------------

/// A symbol row as read from SQLite.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolRow {
    pub id: String,
    pub file: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub language: String,
    pub signature: String,
    pub docstring: String,
    pub summary: String,
    pub decorators_json: String,
    pub keywords_json: String,
    pub parent: Option<String>,
    pub line: i64,
    pub end_line: i64,
    pub byte_offset: i64,
    pub byte_length: i64,
    pub content_hash: String,
}

/// Repository info for list_repos.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoInfo {
    pub repo: String,
    pub indexed_at: String,
    pub symbol_count: usize,
    pub file_count: usize,
    pub languages: HashMap<String, usize>,
    pub index_version: i64,
}

// ---------------------------------------------------------------------------
// Token savings tracker
// ---------------------------------------------------------------------------

pub struct TokenTracker {
    savings_path: PathBuf,
}

impl TokenTracker {
    pub fn new(base_path: Option<&Path>) -> Self {
        let base = match base_path {
            Some(p) => p.to_path_buf(),
            None => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".code-index")
            }
        };
        let _ = std::fs::create_dir_all(&base);
        Self {
            savings_path: base.join("_savings.json"),
        }
    }

    /// Add tokens_saved to running total. Returns new cumulative total.
    pub fn record_savings(&self, tokens_saved: usize) -> usize {
        let mut data = self.load_data();
        let total = data.get("total_tokens_saved")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize
            + tokens_saved;
        data.insert("total_tokens_saved".to_string(), serde_json::json!(total));
        let _ = std::fs::write(&self.savings_path, serde_json::to_string(&data).unwrap_or_default());
        total
    }

    /// Get current cumulative total without modifying it.
    pub fn get_total_saved(&self) -> usize {
        self.load_data()
            .get("total_tokens_saved")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize
    }

    fn load_data(&self) -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(&self.savings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

/// Estimate tokens saved.
pub fn estimate_savings(raw_bytes: usize, response_bytes: usize) -> usize {
    raw_bytes.saturating_sub(response_bytes) / BYTES_PER_TOKEN
}

// ---------------------------------------------------------------------------
// Timestamp helper (no chrono dep — just use strftime-like via std)
// ---------------------------------------------------------------------------

fn chrono_now() -> String {
    // Use system command to avoid pulling in chrono crate.
    std::process::Command::new("date")
        .args(["--iso-8601=seconds"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// rusqlite optional() helper
// ---------------------------------------------------------------------------

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::symbols::Symbol;
    use tempfile::TempDir;

    fn make_symbol(id: &str, name: &str, kind: &str, file: &str) -> Symbol {
        Symbol {
            id: id.to_string(),
            file: file.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: kind.to_string(),
            language: "python".to_string(),
            signature: format!("def {name}():"),
            summary: format!("{kind} {name}"),
            byte_offset: 0,
            byte_length: 100,
            line: 1,
            end_line: 5,
            ..Default::default()
        }
    }

    #[test]
    fn test_save_and_list_repos() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        let symbols = vec![make_symbol("test.py::foo#function", "foo", "function", "test.py")];
        store
            .save_index(
                "owner1",
                "repo1",
                &["test.py".to_string()],
                &symbols,
                &[("test.py".to_string(), "def foo(): pass".to_string())]
                    .into_iter()
                    .collect(),
                &[("python".to_string(), 1)]
                    .into_iter()
                    .collect(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        let repos = store.list_repos().unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].repo, "owner1/repo1");
    }

    #[test]
    fn test_save_and_retrieve_symbol_content() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        let content = "line1\nline2\ndef foo():\n    pass\n";
        let symbols = vec![Symbol {
            id: "test.py::foo#function".to_string(),
            file: "test.py".to_string(),
            name: "foo".to_string(),
            qualified_name: "foo".to_string(),
            kind: "function".to_string(),
            language: "python".to_string(),
            signature: "def foo():".to_string(),
            summary: "function foo".to_string(),
            byte_offset: 12,
            byte_length: 19,
            line: 3,
            end_line: 4,
            ..Default::default()
        }];

        store
            .save_index(
                "test",
                "repo",
                &["test.py".to_string()],
                &symbols,
                &[("test.py".to_string(), content.to_string())]
                    .into_iter()
                    .collect(),
                &[("python".to_string(), 1)]
                    .into_iter()
                    .collect(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        let source = store
            .get_symbol_content("test", "repo", "test.py::foo#function")
            .unwrap();
        assert!(source.is_some());
        assert!(source.unwrap().contains("def foo():"));
    }

    #[test]
    fn test_delete_index() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        store
            .save_index(
                "test",
                "repo",
                &["main.py".to_string()],
                &[],
                &[("main.py".to_string(), "".to_string())]
                    .into_iter()
                    .collect(),
                &HashMap::new(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        assert!(store.index_exists("test", "repo"));
        assert!(store.delete_index("test", "repo").unwrap());
        assert!(!store.index_exists("test", "repo"));
    }

    #[test]
    fn test_fts_search() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        let content = "def authenticate(token): pass\ndef login(user): pass\nclass UserService: pass\n";
        let ts_lang = crate::parser::languages::ts_language_for("python").unwrap();
        let symbols = crate::parser::extractor::parse_file(content, "app.py", "python", ts_lang);

        let mut symbols_with_summaries = symbols;
        crate::summarizer::summarize_symbols_simple(&mut symbols_with_summaries);

        store
            .save_index(
                "o",
                "r",
                &["app.py".to_string()],
                &symbols_with_summaries,
                &[("app.py".to_string(), content.to_string())]
                    .into_iter()
                    .collect(),
                &[("python".to_string(), 1)]
                    .into_iter()
                    .collect(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        let results = store.search_fts("o", "r", "authenticate", None, None, 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "authenticate");
    }

    #[test]
    fn test_fts_kind_filter() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        let content = "def get_user(): pass\nclass User: pass\n";
        let ts_lang = crate::parser::languages::ts_language_for("python").unwrap();
        let mut symbols = crate::parser::extractor::parse_file(content, "svc.py", "python", ts_lang);
        crate::summarizer::summarize_symbols_simple(&mut symbols);

        store
            .save_index(
                "o",
                "r",
                &["svc.py".to_string()],
                &symbols,
                &[("svc.py".to_string(), content.to_string())]
                    .into_iter()
                    .collect(),
                &[("python".to_string(), 1)]
                    .into_iter()
                    .collect(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        let results = store
            .search_fts("o", "r", "user", Some("class"), None, 10)
            .unwrap();
        assert!(results.iter().all(|r| r.kind == "class"));
        assert!(results.iter().any(|r| r.name == "User"));
    }

    #[test]
    fn test_fts_special_chars_no_crash() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        let content = "def foo(): pass\n";
        let ts_lang = crate::parser::languages::ts_language_for("python").unwrap();
        let mut symbols = crate::parser::extractor::parse_file(content, "a.py", "python", ts_lang);
        crate::summarizer::summarize_symbols_simple(&mut symbols);

        store
            .save_index(
                "o",
                "r",
                &["a.py".to_string()],
                &symbols,
                &[("a.py".to_string(), content.to_string())]
                    .into_iter()
                    .collect(),
                &[("python".to_string(), 1)]
                    .into_iter()
                    .collect(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        let bad_queries = ["auth*", "\"broken", "(query)", "a - b", ""];
        for q in &bad_queries {
            let results = store.search_fts("o", "r", q, None, None, 10).unwrap();
            // Should not crash
            let _ = results; // should not crash
        }
    }

    #[test]
    fn test_incremental_detect_changes() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        let content = "def foo(): pass\n";
        let ts_lang = crate::parser::languages::ts_language_for("python").unwrap();
        let mut symbols = crate::parser::extractor::parse_file(content, "a.py", "python", ts_lang);
        crate::summarizer::summarize_symbols_simple(&mut symbols);

        store
            .save_index(
                "o",
                "r",
                &["a.py".to_string()],
                &symbols,
                &[("a.py".to_string(), content.to_string())]
                    .into_iter()
                    .collect(),
                &[("python".to_string(), 1)]
                    .into_iter()
                    .collect(),
                None,
                None,
                &[],
                &[],
                &[],
            )
            .unwrap();

        // No changes
        let current = [("a.py".to_string(), content.to_string())]
            .into_iter()
            .collect();
        let (changed, new_files, deleted) = store.detect_changes("o", "r", &current).unwrap();
        assert!(changed.is_empty());
        assert!(new_files.is_empty());
        assert!(deleted.is_empty());

        // Modified file
        let mut modified = HashMap::new();
        modified.insert("a.py".to_string(), "def foo(): return 1\n".to_string());
        let (changed, new_files, deleted) = store.detect_changes("o", "r", &modified).unwrap();
        assert_eq!(changed.len(), 1);
        assert!(new_files.is_empty());
        assert!(deleted.is_empty());

        // New file added
        let mut with_new = HashMap::new();
        with_new.insert("a.py".to_string(), content.to_string());
        with_new.insert("b.py".to_string(), "def bar(): pass\n".to_string());
        let (changed, new_files, deleted) = store.detect_changes("o", "r", &with_new).unwrap();
        assert!(changed.is_empty());
        assert_eq!(new_files.len(), 1);
        assert!(deleted.is_empty());

        // File deleted
        let empty: HashMap<String, String> = HashMap::new();
        let (changed, new_files, deleted) = store.detect_changes("o", "r", &empty).unwrap();
        assert!(changed.is_empty());
        assert!(new_files.is_empty());
        assert_eq!(deleted.len(), 1);
    }

    #[test]
    fn test_safe_content_path_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let store = IndexStore::open_store(Some(tmp.path())).unwrap();

        // save_index with traversal path should fail
        let result = store.save_index(
            "evil",
            "repo",
            &["../../escape.py".to_string()],
            &[],
            &[("../../escape.py".to_string(), "print('x')".to_string())]
                .into_iter()
                .collect(),
            &HashMap::new(),
            None,
            None,
            &[],
            &[],
            &[],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_safe_repo_component_rejects_traversal() {
        let tmp = TempDir::new().unwrap();

        let result = IndexStore::open_store(Some(tmp.path()))
            .unwrap()
            .save_index(
                "../escape",
                "repo",
                &[],
                &[],
                &HashMap::new(),
                &HashMap::new(),
                None,
                None,
                &[],
                &[],
                &[],
            );
        assert!(result.is_err());
    }
}
