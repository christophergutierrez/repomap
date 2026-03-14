//! MCP tool implementations.
//!
//! Each tool function takes parsed arguments and returns a serde_json::Value.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{json, Value};

use crate::storage::IndexStore;

// ---------------------------------------------------------------------------
// Repo resolution
// ---------------------------------------------------------------------------

/// Public wrapper for resolve_repo, used by mcp.rs for stats estimation.
pub fn resolve_repo_pub(repo: &str, store: &IndexStore) -> Result<(String, String)> {
    store.resolve_repo(repo)
}

// ---------------------------------------------------------------------------
// list_repos
// ---------------------------------------------------------------------------

pub fn list_repos(store: &IndexStore) -> Value {
    let start = Instant::now();
    match store.list_repos() {
        Ok(repos) => {
            let count = repos.len();
            json!({
                "count": count,
                "repos": repos,
                "_meta": { "timing_ms": elapsed_ms(start) }
            })
        }
        Err(e) => json!({"error": e.to_string()}),
    }
}

// ---------------------------------------------------------------------------
// get_repo_outline
// ---------------------------------------------------------------------------

pub fn get_repo_outline(repo: &str, store: &IndexStore) -> Value {
    let start = Instant::now();
    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    // Load metadata.
    let meta = conn.query_row(
        "SELECT indexed_at, source_files, languages FROM repo_meta LIMIT 1",
        [],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)),
    );
    let (indexed_at, files_json, langs_json) = match meta {
        Ok(m) => m,
        Err(_) => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let source_files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();
    let languages: HashMap<String, usize> = serde_json::from_str(&langs_json).unwrap_or_default();

    let symbol_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))
        .unwrap_or(0);

    // Directory breakdown.
    let mut dir_counts: HashMap<String, usize> = HashMap::new();
    for f in &source_files {
        let key = match f.split_once('/') {
            Some((dir, _)) => format!("{dir}/"),
            None => "(root)".to_string(),
        };
        *dir_counts.entry(key).or_default() += 1;
    }

    // Symbol kind breakdown.
    let mut kind_counts: HashMap<String, usize> = HashMap::new();
    let mut stmt = conn.prepare("SELECT kind, COUNT(*) FROM symbols GROUP BY kind").unwrap();
    let mut rows = stmt.query([]).unwrap();
    while let Ok(Some(r)) = rows.next() {
        let kind: String = r.get(0).unwrap_or_default();
        let count: i64 = r.get(1).unwrap_or(0);
        kind_counts.insert(kind, count as usize);
    }

    json!({
        "repo": format!("{owner}/{name}"),
        "indexed_at": indexed_at,
        "file_count": source_files.len(),
        "symbol_count": symbol_count,
        "languages": languages,
        "directories": dir_counts,
        "symbol_kinds": kind_counts,
        "_meta": { "timing_ms": elapsed_ms(start) }
    })
}

// ---------------------------------------------------------------------------
// get_file_tree
// ---------------------------------------------------------------------------

pub fn get_file_tree(repo: &str, path_prefix: &str, store: &IndexStore) -> Value {
    let start = Instant::now();
    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let files_json: String = conn
        .query_row("SELECT source_files FROM repo_meta LIMIT 1", [], |r| r.get(0))
        .unwrap_or_else(|_| "[]".to_string());
    let source_files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();

    let files: Vec<&String> = source_files
        .iter()
        .filter(|f| f.starts_with(path_prefix))
        .collect();

    if files.is_empty() {
        return json!({
            "repo": format!("{owner}/{name}"),
            "path_prefix": path_prefix,
            "tree": []
        });
    }

    // Build nested tree.
    let tree = build_file_tree(&files, &conn, path_prefix);

    json!({
        "repo": format!("{owner}/{name}"),
        "path_prefix": path_prefix,
        "tree": tree,
        "_meta": {
            "timing_ms": elapsed_ms(start),
            "file_count": files.len()
        }
    })
}

fn build_file_tree(files: &[&String], conn: &Connection, path_prefix: &str) -> Vec<Value> {
    // Build nested HashMap structure, then convert to sorted JSON.
    let mut root: serde_json::Map<String, Value> = serde_json::Map::new();

    for file_path in files {
        let rel = file_path
            .strip_prefix(path_prefix)
            .unwrap_or(file_path)
            .trim_start_matches('/');
        let parts: Vec<&str> = rel.split('/').collect();

        let mut current = &mut root;
        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;
            if is_last {
                let sym_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM symbols WHERE file = ?",
                        [file_path.as_str()],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);

                let lang = crate::parser::languages::language_for_extension(Path::new(file_path))
                    .unwrap_or("");

                current.insert(
                    part.to_string(),
                    json!({
                        "path": file_path,
                        "type": "file",
                        "language": lang,
                        "symbol_count": sym_count
                    }),
                );
            } else {
                if !current.contains_key(*part) {
                    current.insert(
                        part.to_string(),
                        json!({"type": "dir", "children": {}}),
                    );
                }
                current = current
                    .get_mut(*part)
                    .unwrap()
                    .get_mut("children")
                    .unwrap()
                    .as_object_mut()
                    .unwrap();
            }
        }
    }

    dict_to_tree_list(&root)
}

fn dict_to_tree_list(map: &serde_json::Map<String, Value>) -> Vec<Value> {
    let mut entries: Vec<(&String, &Value)> = map.iter().collect();
    entries.sort_by_key(|(k, _)| k.to_lowercase());

    entries
        .into_iter()
        .map(|(name, node)| {
            if node.get("type").and_then(|v| v.as_str()) == Some("file") {
                node.clone()
            } else {
                let children = node
                    .get("children")
                    .and_then(|c| c.as_object())
                    .map(dict_to_tree_list)
                    .unwrap_or_default();
                json!({
                    "path": format!("{name}/"),
                    "type": "dir",
                    "children": children
                })
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// get_file_outline
// ---------------------------------------------------------------------------

pub fn get_file_outline(repo: &str, file_path: &str, store: &IndexStore) -> Value {
    let start = Instant::now();
    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let mut stmt = conn
        .prepare(
            "SELECT id, name, kind, signature, summary, line, parent
             FROM symbols WHERE file = ? ORDER BY line",
        )
        .unwrap();

    let symbols: Vec<(String, String, String, String, String, i64, Option<String>)> = stmt
        .query_map([file_path], |r| {
            Ok((
                r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?,
                r.get(4)?, r.get(5)?, r.get(6)?,
            ))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    if symbols.is_empty() {
        return json!({
            "repo": format!("{owner}/{name}"),
            "file": file_path,
            "language": "",
            "symbols": []
        });
    }

    // Build hierarchy: top-level symbols + children.
    let mut top_level = Vec::new();
    let mut children_map: HashMap<String, Vec<Value>> = HashMap::new();

    for (id, sym_name, kind, sig, summary, line, parent) in &symbols {
        let node = json!({
            "id": id,
            "kind": kind,
            "name": sym_name,
            "signature": sig,
            "summary": summary,
            "line": line,
        });
        match parent {
            Some(p) => children_map.entry(p.clone()).or_default().push(node),
            None => top_level.push((id.clone(), node)),
        }
    }

    // Attach children.
    let outline: Vec<Value> = top_level
        .into_iter()
        .map(|(id, mut node)| {
            if let Some(kids) = children_map.remove(&id) {
                node.as_object_mut().unwrap().insert("children".to_string(), json!(kids));
            }
            node
        })
        .collect();

    // Get language from first symbol.
    let language = conn
        .query_row(
            "SELECT language FROM symbols WHERE file = ? LIMIT 1",
            [file_path],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default();

    json!({
        "repo": format!("{owner}/{name}"),
        "file": file_path,
        "language": language,
        "symbols": outline,
        "_meta": {
            "timing_ms": elapsed_ms(start),
            "symbol_count": symbols.len()
        }
    })
}

// ---------------------------------------------------------------------------
// get_symbol
// ---------------------------------------------------------------------------

pub fn get_symbol(repo: &str, symbol_id: &str, store: &IndexStore) -> Value {
    let start = Instant::now();
    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let row = conn.query_row(
        "SELECT id, file, name, kind, signature, docstring, decorators,
                line, end_line, byte_offset, byte_length, content_hash
         FROM symbols WHERE id = ?",
        [symbol_id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, i64>(10)?,
                r.get::<_, String>(11)?,
            ))
        },
    );

    let (id, file, sym_name, kind, sig, docstring, decorators_json, line, end_line, _byte_offset, _byte_length, content_hash) =
        match row {
            Ok(r) => r,
            Err(_) => return json!({"error": format!("Symbol not found: {symbol_id}")}),
        };

    let source = store
        .get_symbol_content(&owner, &name, symbol_id)
        .ok()
        .flatten()
        .unwrap_or_default();

    let decorators: Vec<String> = serde_json::from_str(&decorators_json).unwrap_or_default();

    json!({
        "id": id,
        "kind": kind,
        "name": sym_name,
        "file": file,
        "line": line,
        "end_line": end_line,
        "signature": sig,
        "decorators": decorators,
        "docstring": docstring,
        "content_hash": content_hash,
        "source": source,
        "_meta": { "timing_ms": elapsed_ms(start) }
    })
}

// ---------------------------------------------------------------------------
// get_symbols (batch)
// ---------------------------------------------------------------------------

pub fn get_symbols(repo: &str, symbol_ids: &[String], store: &IndexStore) -> Value {
    let start = Instant::now();
    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let mut symbols = Vec::new();
    let mut errors = Vec::new();

    for sid in symbol_ids {
        let row = conn.query_row(
            "SELECT id, file, name, kind, signature, docstring, decorators,
                    line, end_line, content_hash
             FROM symbols WHERE id = ?",
            [sid.as_str()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?, r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?, r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?, r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,    r.get::<_, String>(9)?,
                ))
            },
        );

        match row {
            Ok((id, file, sym_name, kind, sig, docstring, dec_json, line, end_line, hash)) => {
                let source = store.get_symbol_content(&owner, &name, sid).ok().flatten().unwrap_or_default();
                let decorators: Vec<String> = serde_json::from_str(&dec_json).unwrap_or_default();
                symbols.push(json!({
                    "id": id, "kind": kind, "name": sym_name, "file": file,
                    "line": line, "end_line": end_line, "signature": sig,
                    "decorators": decorators, "docstring": docstring,
                    "content_hash": hash, "source": source,
                }));
            }
            Err(_) => errors.push(json!({"id": sid, "error": format!("Symbol not found: {sid}")})),
        }
    }

    json!({
        "symbols": symbols,
        "errors": errors,
        "_meta": { "timing_ms": elapsed_ms(start), "symbol_count": symbols.len() }
    })
}

// ---------------------------------------------------------------------------
// search_symbols
// ---------------------------------------------------------------------------

pub fn search_symbols(
    repo: &str,
    query: &str,
    kind: Option<&str>,
    language: Option<&str>,
    max_results: usize,
    store: &IndexStore,
) -> Value {
    let start = Instant::now();
    let max_results = max_results.clamp(1, 100);

    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let results = match store.search_fts(&owner, &name, query, kind, language, max_results) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    let scored: Vec<Value> = results
        .iter()
        .map(|sym| {
            let score = calculate_score(&sym.name, &sym.signature, &sym.summary, &sym.docstring, &query_lower, &query_words);
            json!({
                "id": sym.id,
                "kind": sym.kind,
                "name": sym.name,
                "file": sym.file,
                "line": sym.line,
                "signature": sym.signature,
                "summary": sym.summary,
                "score": score
            })
        })
        .collect();

    json!({
        "repo": format!("{owner}/{name}"),
        "query": query,
        "result_count": scored.len(),
        "results": scored,
        "_meta": { "timing_ms": elapsed_ms(start) }
    })
}

fn calculate_score(name: &str, sig: &str, summary: &str, docstring: &str, query_lower: &str, query_words: &[&str]) -> i32 {
    let mut score = 0i32;
    let name_lower = name.to_lowercase();

    if query_lower == name_lower {
        score += 20;
    } else if name_lower.contains(query_lower) {
        score += 10;
    }
    for w in query_words {
        if name_lower.contains(w) { score += 5; }
    }

    let sig_lower = sig.to_lowercase();
    if sig_lower.contains(query_lower) { score += 8; }
    for w in query_words {
        if sig_lower.contains(w) { score += 2; }
    }

    let sum_lower = summary.to_lowercase();
    if sum_lower.contains(query_lower) { score += 5; }
    for w in query_words {
        if sum_lower.contains(w) { score += 1; }
    }

    let doc_lower = docstring.to_lowercase();
    for w in query_words {
        if doc_lower.contains(w) { score += 1; }
    }

    score
}

// ---------------------------------------------------------------------------
// search_text
// ---------------------------------------------------------------------------

pub fn search_text(repo: &str, query: &str, file_pattern: Option<&str>, max_results: usize, store: &IndexStore) -> Value {
    let start = Instant::now();
    let max_results = max_results.clamp(1, 100);

    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let db_path = match store.index_path_pub(&owner, &name) {
        Ok(p) if p.exists() => p,
        _ => return json!({"error": format!("Repository not indexed: {owner}/{name}")}),
    };

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let files_json: String = conn
        .query_row("SELECT source_files FROM repo_meta LIMIT 1", [], |r| r.get(0))
        .unwrap_or_else(|_| "[]".to_string());
    let source_files: Vec<String> = serde_json::from_str(&files_json).unwrap_or_default();

    let content_dir = match store.content_dir_pub(&owner, &name) {
        Ok(d) => d,
        Err(e) => return json!({"error": e.to_string()}),
    };

    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();
    let mut files_searched = 0usize;

    for file_path in &source_files {
        if let Some(pat) = file_pattern {
            if !file_path.contains(pat) && !file_path.ends_with(pat) {
                continue;
            }
        }

        let full = content_dir.join(file_path);
        let content = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(_) => continue,
        };
        files_searched += 1;

        for (line_num, line) in content.lines().enumerate() {
            if line.to_lowercase().contains(&query_lower) {
                let text = if line.len() > 200 { &line[..line.floor_char_boundary(200)] } else { line };
                matches.push(json!({
                    "file": file_path,
                    "line": line_num + 1,
                    "text": text.trim_end()
                }));
                if matches.len() >= max_results {
                    break;
                }
            }
        }
        if matches.len() >= max_results {
            break;
        }
    }

    json!({
        "repo": format!("{owner}/{name}"),
        "query": query,
        "result_count": matches.len(),
        "results": matches,
        "_meta": {
            "timing_ms": elapsed_ms(start),
            "files_searched": files_searched,
            "truncated": matches.len() >= max_results
        }
    })
}

// ---------------------------------------------------------------------------
// invalidate_cache
// ---------------------------------------------------------------------------

pub fn invalidate_cache(repo: &str, store: &IndexStore) -> Value {
    let (owner, name) = match store.resolve_repo(repo) {
        Ok(r) => r,
        Err(e) => return json!({"error": e.to_string()}),
    };

    match store.delete_index(&owner, &name) {
        Ok(true) => json!({
            "success": true,
            "repo": format!("{owner}/{name}"),
            "message": format!("Index and cached files deleted for {owner}/{name}")
        }),
        Ok(false) => json!({
            "success": false,
            "error": format!("No index found for {owner}/{name}")
        }),
        Err(e) => json!({"error": e.to_string()}),
    }
}

// ---------------------------------------------------------------------------
// index_folder
// ---------------------------------------------------------------------------

pub async fn index_folder(path_str: &str, store: &IndexStore) -> Value {
    let start = Instant::now();

    let path = match std::path::PathBuf::from(path_str).canonicalize() {
        Ok(p) => p,
        Err(e) => return json!({"error": format!("Invalid path: {e}")}),
    };

    if !path.is_dir() {
        return json!({"error": format!("Not a directory: {}", path.display())});
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let owner = "local";

    let files = match crate::discovery::discover_files(&path) {
        Ok(f) => f,
        Err(e) => return json!({"error": format!("Discovery failed: {e}")}),
    };

    let mut parsed = match crate::parser::parse_files(&files, &path) {
        Ok(s) => s,
        Err(e) => return json!({"error": format!("Parse failed: {e}")}),
    };

    // Summarize without AI (fast path for MCP tool).
    crate::summarizer::summarize_symbols_simple(&mut parsed.symbols);

    // Read all file contents.
    let mut raw_files: HashMap<String, String> = HashMap::new();
    let mut source_file_list: Vec<String> = Vec::new();
    for fp in &files {
        let rel = fp.strip_prefix(&path).unwrap_or(fp);
        let rel_str = rel.to_string_lossy().to_string();
        if let Ok(content) = std::fs::read_to_string(fp) {
            raw_files.insert(rel_str.clone(), content);
            source_file_list.push(rel_str);
        }
    }
    source_file_list.sort();

    let languages = crate::parser::languages::count_languages_from_files(&raw_files);

    match store.save_index(
        owner,
        &name,
        &source_file_list,
        &parsed.symbols,
        &raw_files,
        &languages,
        None,
        Some(&path),
        &parsed.imports,
        &parsed.proto_refs,
        &parsed.impl_refs,
    ) {
        Ok(()) => json!({
            "success": true,
            "repo": format!("{owner}/{name}"),
            "file_count": source_file_list.len(),
            "symbol_count": parsed.symbols.len(),
            "_meta": { "timing_ms": elapsed_ms(start) }
        }),
        Err(e) => json!({"error": format!("Index save failed: {e}")}),
    }
}

// ---------------------------------------------------------------------------
// index_repo
// ---------------------------------------------------------------------------

pub async fn index_repo(
    path_str: &str,
    use_ai: bool,
    store: &IndexStore,
) -> Value {
    let start = Instant::now();

    let path = match std::path::PathBuf::from(path_str).canonicalize() {
        Ok(p) => p,
        Err(e) => return json!({"error": format!("Invalid path: {e}")}),
    };

    if !path.is_dir() {
        return json!({"error": format!("Not a directory: {}", path.display())});
    }

    // Derive owner/repo from git remote, fall back to local/<dirname>.
    let (owner, repo_name) = git_remote_owner_repo(&path)
        .unwrap_or_else(|| {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string());
            ("local".to_string(), name)
        });

    let files = match crate::discovery::discover_files(&path) {
        Ok(f) => f,
        Err(e) => return json!({"error": format!("Discovery failed: {e}")}),
    };

    let mut parsed = match crate::parser::parse_files(&files, &path) {
        Ok(s) => s,
        Err(e) => return json!({"error": format!("Parse failed: {e}")}),
    };

    // Summarize.
    if use_ai {
        crate::summarizer::summarize_symbols(&mut parsed.symbols, true).await;
    } else {
        crate::summarizer::summarize_symbols_simple(&mut parsed.symbols);
    }

    // Read file contents.
    let mut raw_files: HashMap<String, String> = HashMap::new();
    let mut source_file_list: Vec<String> = Vec::new();
    for fp in &files {
        let rel = fp.strip_prefix(&path).unwrap_or(fp);
        let rel_str = rel.to_string_lossy().to_string();
        if let Ok(content) = std::fs::read_to_string(fp) {
            raw_files.insert(rel_str.clone(), content);
            source_file_list.push(rel_str);
        }
    }
    source_file_list.sort();

    let languages = crate::parser::languages::count_languages_from_files(&raw_files);

    match store.save_index(
        &owner,
        &repo_name,
        &source_file_list,
        &parsed.symbols,
        &raw_files,
        &languages,
        None,
        Some(&path),
        &parsed.imports,
        &parsed.proto_refs,
        &parsed.impl_refs,
    ) {
        Ok(()) => json!({
            "success": true,
            "repo": format!("{owner}/{repo_name}"),
            "file_count": source_file_list.len(),
            "symbol_count": parsed.symbols.len(),
            "languages": languages,
            "_meta": { "timing_ms": elapsed_ms(start) }
        }),
        Err(e) => json!({"error": format!("Index save failed: {e}")}),
    }
}

/// Extract owner/repo from git remote origin URL.
fn git_remote_owner_repo(path: &Path) -> Option<(String, String)> {
    let output = std::process::Command::new("git")
        .args(["-C", &path.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_github_url(&url).ok()
}

/// Parse GitHub URL or owner/repo string into (owner, repo).
fn parse_github_url(url: &str) -> std::result::Result<(String, String), String> {
    let url = url.trim_end_matches(".git");

    // SSH format: git@github.com:owner/repo
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // HTTPS format: https://github.com/owner/repo
    if let Some(path) = url.strip_prefix("https://github.com/") {
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    // Simple owner/repo format.
    if url.contains('/') && !url.contains("://") && !url.contains(':') {
        let parts: Vec<&str> = url.split('/').collect();
        if parts.len() >= 2 {
            return Ok((parts[0].to_string(), parts[1].to_string()));
        }
    }

    Err(format!("Could not parse GitHub URL: {url}"))
}

/// Fetch full repository tree via GitHub git/trees API.
async fn fetch_repo_tree(
    owner: &str,
    repo: &str,
    token: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    let url = format!(
        "https://api.github.com/repos/{owner}/{repo}/git/trees/HEAD?recursive=1"
    );

    let client = reqwest::Client::new();
    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "repomap");

    if let Some(token) = token {
        req = req.header("Authorization", format!("token {token}"));
    }

    let resp = req.send().await?.error_for_status()?;
    let body: serde_json::Value = resp.json().await?;
    let tree = body["tree"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(tree)
}

/// Fetch raw file content from GitHub.
async fn fetch_file_content(
    owner: &str,
    repo: &str,
    path: &str,
    token: Option<&str>,
) -> Result<String> {
    let client = reqwest::Client::new();
    fetch_file_content_with_client(&client, owner, repo, path, token).await
}

async fn fetch_file_content_with_client(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
    path: &str,
    token: Option<&str>,
) -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{owner}/{repo}/contents/{path}"
    );

    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github.v3.raw")
        .header("User-Agent", "repomap");

    if let Some(token) = token {
        req = req.header("Authorization", format!("token {token}"));
    }

    let resp = req.send().await?.error_for_status()?;
    Ok(resp.text().await?)
}

/// Skip patterns for remote file discovery.
const REMOTE_SKIP_PATTERNS: &[&str] = &[
    "node_modules/",
    "vendor/",
    "venv/",
    ".venv/",
    "__pycache__/",
    "dist/",
    "build/",
    ".git/",
    ".tox/",
    ".mypy_cache/",
    "target/",
    ".gradle/",
    "test_data/",
    "testdata/",
    "fixtures/",
    "snapshots/",
    "migrations/",
    ".min.js",
    ".min.ts",
    ".bundle.js",
    "package-lock.json",
    "yarn.lock",
    "go.sum",
    "generated/",
];

/// Discover source files from GitHub tree entries.
fn discover_remote_source_files(
    tree_entries: &[serde_json::Value],
    gitignore_content: Option<&str>,
) -> Vec<String> {
    let max_files = 500;
    let max_size = 500 * 1024;

    let mut files = Vec::new();

    for entry in tree_entries {
        if entry["type"].as_str() != Some("blob") {
            continue;
        }

        let path = match entry["path"].as_str() {
            Some(p) => p,
            None => continue,
        };
        let size = entry["size"].as_u64().unwrap_or(0);

        // Extension filter.
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        if !crate::discovery::INDEXABLE_EXTENSIONS.contains(&ext) {
            continue;
        }

        // Skip patterns.
        if REMOTE_SKIP_PATTERNS.iter().any(|p| path.contains(p)) {
            continue;
        }

        // Secret detection.
        if crate::discovery::is_secret_file(path) {
            continue;
        }

        // Binary check.
        if crate::discovery::is_binary_extension(Path::new(path)) {
            continue;
        }

        // Size limit.
        if size > max_size {
            continue;
        }

        // TODO: gitignore matching (would need a pathspec crate).
        let _ = gitignore_content;

        files.push(path.to_string());
    }

    // Prioritize and limit.
    if files.len() > max_files {
        let priority_dirs = ["src/", "lib/", "pkg/", "cmd/", "internal/"];
        files.sort_by(|a, b| {
            let a_prio = priority_dirs
                .iter()
                .position(|d| a.starts_with(d))
                .unwrap_or(priority_dirs.len());
            let b_prio = priority_dirs
                .iter()
                .position(|d| b.starts_with(d))
                .unwrap_or(priority_dirs.len());
            a_prio
                .cmp(&b_prio)
                .then_with(|| a.matches('/').count().cmp(&b.matches('/').count()))
                .then_with(|| a.cmp(b))
        });
        files.truncate(max_files);
    }

    files
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
